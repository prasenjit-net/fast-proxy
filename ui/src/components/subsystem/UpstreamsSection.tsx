import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { IconEdit, IconPlus, IconTrash } from "../../icons";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { SubsystemDetail, Upstream, UpstreamInput } from "../../lib/types";
import ConfirmDialog from "../ConfirmDialog";
import UpstreamForm from "../forms/UpstreamForm";
import Modal from "../Modal";

type Editing = { kind: "new" } | { kind: "edit"; upstream: Upstream };

/** The backends a subsystem's routes can send traffic to. */
export default function UpstreamsSection({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [editing, setEditing] = useState<Editing | null>(null);
  const [deleting, setDeleting] = useState<Upstream | null>(null);
  const [busy, setBusy] = useState(false);

  const routesUsing = (upstream: Upstream) =>
    subsystem.routes.filter((route) => route.upstreamGroupId === upstream.id).length;

  async function save(input: UpstreamInput) {
    if (editing?.kind === "edit") {
      await api.updateUpstream(subsystem.id, editing.upstream.id, input);
      push("success", `Updated upstream ${input.name}`);
    } else {
      await api.createUpstream(subsystem.id, input);
      push("success", `Added upstream ${input.name}`);
    }
    await refresh();
    setEditing(null);
  }

  async function remove(upstream: Upstream) {
    setBusy(true);
    try {
      await api.deleteUpstream(subsystem.id, upstream.id);
      await refresh();
      push("success", `Deleted upstream ${upstream.name}`);
      setDeleting(null);
    } catch (error) {
      notifyError(error);
      setDeleting(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Upstreams</h2>
        <span className="card-hint">where routes send traffic</span>
        <button className="btn btn-secondary btn-sm ml-auto" onClick={() => setEditing({ kind: "new" })}>
          <IconPlus size={14} /> Add upstream
        </button>
      </div>
      {subsystem.upstreams.length === 0 ? (
        <p className="text-[0.88rem] text-ink-muted">
          No upstreams yet. Add the address of the backend server first; routes point at it.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[0.88rem]">
            <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
              <tr>
                <th className="py-2 pr-4 font-medium">Name</th>
                <th className="py-2 pr-4 font-medium">Address</th>
                <th className="py-2 pr-4 font-medium">Used by</th>
                <th className="py-2" />
              </tr>
            </thead>
            <tbody>
              {subsystem.upstreams.map((upstream) => (
                <tr key={upstream.id} className="border-t border-line">
                  <td className="py-2 pr-4 font-medium">{upstream.name}</td>
                  <td className="py-2 pr-4 font-mono text-[0.82rem]">{upstream.addr}</td>
                  <td className="py-2 pr-4 text-ink-muted">
                    {routesUsing(upstream)} route{routesUsing(upstream) === 1 ? "" : "s"}
                  </td>
                  <td className="py-2 text-right whitespace-nowrap">
                    <button
                      className="icon-btn"
                      onClick={() => setEditing({ kind: "edit", upstream })}
                      aria-label={`Edit upstream ${upstream.name}`}
                    >
                      <IconEdit size={16} />
                    </button>
                    <button
                      className="icon-btn danger"
                      onClick={() => setDeleting(upstream)}
                      aria-label={`Delete upstream ${upstream.name}`}
                    >
                      <IconTrash size={16} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {editing ? (
        <Modal
          title={editing.kind === "edit" ? `Edit upstream ${editing.upstream.name}` : "Add upstream"}
          onClose={() => setEditing(null)}
        >
          <UpstreamForm
            initial={editing.kind === "edit" ? editing.upstream : undefined}
            submitLabel={editing.kind === "edit" ? "Save upstream" : "Add upstream"}
            onSubmit={save}
            onCancel={() => setEditing(null)}
          />
        </Modal>
      ) : null}

      {deleting ? (
        <ConfirmDialog
          title="Delete upstream?"
          confirmLabel="Delete upstream"
          busy={busy}
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        >
          <p>
            This removes upstream <b>{deleting.name}</b> ({deleting.addr}).
            {routesUsing(deleting) > 0
              ? ` ${routesUsing(deleting)} route(s) still use it, so the server will refuse until they are changed or deleted.`
              : " No route uses it."}
          </p>
        </ConfirmDialog>
      ) : null}
    </section>
  );
}
