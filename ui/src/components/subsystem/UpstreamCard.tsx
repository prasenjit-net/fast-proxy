import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { IconEdit, IconPlus, IconTrash } from "../../icons";
import { api } from "../../lib/api";
import { LB_ALGORITHMS } from "../../lib/policyValidation";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { EndpointInput, SubsystemDetail, Upstream, UpstreamSettings } from "../../lib/types";
import { useUpstreamStatus } from "../../lib/useUpstreamStatus";
import Badge from "../Badge";
import ConfirmDialog from "../ConfirmDialog";
import EndpointForm from "../forms/EndpointForm";
import UpstreamForm from "../forms/UpstreamForm";
import Modal from "../Modal";
import WriteOnly from "../WriteOnly";
import EndpointTable from "./EndpointTable";

type Dialog = "settings" | "endpoint" | "delete" | null;

/** One upstream group: its policy, a health summary, and its endpoints. */
export default function UpstreamCard({ subsystem, upstream }: { subsystem: SubsystemDetail; upstream: Upstream }) {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [dialog, setDialog] = useState<Dialog>(null);
  const [busy, setBusy] = useState(false);
  const status = useUpstreamStatus(subsystem.id, upstream.id).data?.data;
  const routesUsing = subsystem.routes.filter((route) => route.upstreamGroupId === upstream.id).length;
  const algorithm = LB_ALGORITHMS.find((entry) => entry.value === upstream.lb.algorithm)?.label;
  const total = upstream.endpoints.length;

  async function saveSettings(settings: UpstreamSettings) {
    await api.updateUpstream(subsystem.id, upstream.id, settings);
    await refresh();
    push("success", `Updated upstream ${settings.name}`);
    setDialog(null);
  }

  async function addEndpoint(input: EndpointInput) {
    await api.addEndpoint(subsystem.id, upstream.id, input);
    await refresh();
    push("success", `Added endpoint ${input.addr}`);
    setDialog(null);
  }

  async function remove() {
    setBusy(true);
    try {
      await api.deleteUpstream(subsystem.id, upstream.id);
      await refresh();
      push("success", `Deleted upstream ${upstream.name}`);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
      setDialog(null);
    }
  }

  return (
    <div className="rounded-md border border-line p-3">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <h3 className="text-[0.95rem] font-semibold">{upstream.name}</h3>
        <Badge>{algorithm ?? upstream.lb.algorithm}</Badge>
        {status ? (
          <Badge tone={status.healthy === total ? "ok" : status.healthy === 0 ? "err" : "warn"} dot>
            {status.healthy} of {total} healthy
          </Badge>
        ) : null}
        <span className="text-[0.8rem] text-ink-muted">
          used by {routesUsing} route{routesUsing === 1 ? "" : "s"}
        </span>
        <WriteOnly>
          <span className="ml-auto flex gap-1">
            <button className="btn btn-secondary btn-sm" onClick={() => setDialog("endpoint")}>
              <IconPlus size={14} /> Add endpoint
            </button>
            <button className="icon-btn" onClick={() => setDialog("settings")} aria-label={`Edit upstream ${upstream.name}`}>
              <IconEdit size={16} />
            </button>
            <button className="icon-btn danger" onClick={() => setDialog("delete")} aria-label={`Delete upstream ${upstream.name}`}>
              <IconTrash size={16} />
            </button>
          </span>
        </WriteOnly>
      </div>
      <EndpointTable subsystemId={subsystem.id} upstream={upstream} status={status} />

      {dialog === "settings" ? (
        <Modal title={`Edit upstream ${upstream.name}`} onClose={() => setDialog(null)} wide>
          <UpstreamForm initial={upstream} submitLabel="Save upstream" onSubmit={saveSettings} onCancel={() => setDialog(null)} />
        </Modal>
      ) : null}
      {dialog === "endpoint" ? (
        <Modal title={`Add endpoint to ${upstream.name}`} onClose={() => setDialog(null)}>
          <EndpointForm submitLabel="Add endpoint" onSubmit={addEndpoint} onCancel={() => setDialog(null)} />
        </Modal>
      ) : null}
      {dialog === "delete" ? (
        <ConfirmDialog
          title="Delete upstream?"
          confirmLabel="Delete upstream"
          busy={busy}
          onCancel={() => setDialog(null)}
          onConfirm={() => void remove()}
        >
          <p>
            This removes upstream <b>{upstream.name}</b> and its {total} endpoint(s).
            {routesUsing > 0
              ? ` ${routesUsing} route(s) still use it, so the server will refuse until they are changed or deleted.`
              : " No route uses it."}
          </p>
        </ConfirmDialog>
      ) : null}
    </div>
  );
}
