import { useState } from "react";
import { useCanWrite } from "../../context/SessionContext";
import { useToast } from "../../context/ToastContext";
import { IconEdit, IconPlus, IconTrash } from "../../icons";
import { api } from "../../lib/api";
import { formatDurationMs } from "../../lib/format";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { Route, RouteInput, SubsystemDetail } from "../../lib/types";
import ConfirmDialog from "../ConfirmDialog";
import RouteForm from "../forms/RouteForm";
import Modal from "../Modal";
import Toggle from "../Toggle";
import WriteOnly from "../WriteOnly";

type Editing = { kind: "new" } | { kind: "edit"; route: Route };

function inputOf(route: Route): RouteInput {
  const { id: _id, ...input } = route;
  return input;
}

/** A short description of how a route rewrites the request. */
function rewriteSummary(route: Route): string {
  const parts: string[] = [];
  if (route.stripPrefix) parts.push("strip prefix");
  if (route.upstreamPrefix) parts.push(`prepend ${route.upstreamPrefix}`);
  if (!route.preserveHost) parts.push("rewrite Host");
  return parts.length > 0 ? parts.join(", ") : "none";
}

/** A subsystem's routes, in the order they are matched (longest prefix first). */
export default function RoutesSection({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const canWrite = useCanWrite();
  const [editing, setEditing] = useState<Editing | null>(null);
  const [deleting, setDeleting] = useState<Route | null>(null);
  const [busy, setBusy] = useState(false);

  const upstreamName = (id: number) =>
    subsystem.upstreams.find((upstream) => upstream.id === id)?.name ?? `#${id}`;

  async function save(input: RouteInput) {
    if (editing?.kind === "edit") {
      await api.updateRoute(subsystem.id, editing.route.id, input);
      push("success", `Updated route ${input.prefix}`);
    } else {
      await api.createRoute(subsystem.id, input);
      push("success", `Added route ${input.prefix}`);
    }
    await refresh();
    setEditing(null);
  }

  async function toggle(route: Route, enabled: boolean) {
    try {
      await api.updateRoute(subsystem.id, route.id, { ...inputOf(route), enabled });
      await refresh();
      push("success", `${enabled ? "Enabled" : "Disabled"} route ${route.prefix}`);
    } catch (error) {
      notifyError(error);
    }
  }

  async function remove(route: Route) {
    setBusy(true);
    try {
      await api.deleteRoute(subsystem.id, route.id);
      await refresh();
      push("success", `Deleted route ${route.prefix}`);
      setDeleting(null);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Routes</h2>
        <span className="card-hint">longest matching prefix wins</span>
        <WriteOnly>
          <button className="btn btn-secondary btn-sm ml-auto" onClick={() => setEditing({ kind: "new" })}>
            <IconPlus size={14} /> Add route
          </button>
        </WriteOnly>
      </div>
      {subsystem.routes.length === 0 ? (
        <p className="text-[0.88rem] text-ink-muted">
          No routes yet, so every request to this subsystem's hosts gets a 404. Add a route with prefix{" "}
          <code>/</code> to send everything to an upstream.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[0.88rem]">
            <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
              <tr>
                <th className="py-2 pr-4 font-medium">Prefix</th>
                <th className="py-2 pr-4 font-medium">Upstream</th>
                <th className="py-2 pr-4 font-medium">Rewrite</th>
                <th className="py-2 pr-4 font-medium">Timeouts</th>
                <th className="py-2 pr-4 font-medium">Enabled</th>
                <th className="py-2" />
              </tr>
            </thead>
            <tbody>
              {subsystem.routes.map((route) => (
                <tr key={route.id} className="border-t border-line">
                  <td className="py-2 pr-4 font-mono text-[0.82rem]">{route.prefix}</td>
                  <td className="py-2 pr-4">{upstreamName(route.upstreamGroupId)}</td>
                  <td className="py-2 pr-4 text-ink-muted">{rewriteSummary(route)}</td>
                  <td className="py-2 pr-4 font-mono text-[0.78rem] text-ink-muted">
                    {formatDurationMs(route.connectTimeoutMs)} / {formatDurationMs(route.responseTimeoutMs)}
                  </td>
                  <td className="py-2 pr-4">
                    {canWrite ? (
                      <Toggle
                        checked={route.enabled}
                        onChange={(enabled) => void toggle(route, enabled)}
                        label={route.enabled ? "On" : "Off"}
                      />
                    ) : (
                      <span>{route.enabled ? "On" : "Off"}</span>
                    )}
                  </td>
                  <td className="py-2 text-right whitespace-nowrap">
                    <WriteOnly>
                      <button
                        className="icon-btn"
                        onClick={() => setEditing({ kind: "edit", route })}
                        aria-label={`Edit route ${route.prefix}`}
                      >
                        <IconEdit size={16} />
                      </button>
                      <button
                        className="icon-btn danger"
                        onClick={() => setDeleting(route)}
                        aria-label={`Delete route ${route.prefix}`}
                      >
                        <IconTrash size={16} />
                      </button>
                    </WriteOnly>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {editing ? (
        <Modal
          wide
          title={editing.kind === "edit" ? `Edit route ${editing.route.prefix}` : "Add route"}
          onClose={() => setEditing(null)}
        >
          <RouteForm
            upstreams={subsystem.upstreams}
            initial={editing.kind === "edit" ? inputOf(editing.route) : undefined}
            submitLabel={editing.kind === "edit" ? "Save route" : "Add route"}
            onSubmit={save}
            onCancel={() => setEditing(null)}
          />
        </Modal>
      ) : null}

      {deleting ? (
        <ConfirmDialog
          title="Delete route?"
          confirmLabel="Delete route"
          busy={busy}
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        >
          <p>
            This removes route <b className="font-mono">{deleting.prefix}</b>. Requests under that
            path will use the next-longest matching route, or get a 404 if there is none.
          </p>
        </ConfirmDialog>
      ) : null}
    </section>
  );
}
