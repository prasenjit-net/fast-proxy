import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { IconEdit, IconTrash } from "../../icons";
import { api } from "../../lib/api";
import { formatDurationMs } from "../../lib/format";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { Endpoint, EndpointInput, EndpointState, Upstream, UpstreamStatus } from "../../lib/types";
import ConfirmDialog from "../ConfirmDialog";
import EndpointForm from "../forms/EndpointForm";
import EndpointStateBadge from "../EndpointStateBadge";
import Modal from "../Modal";
import WriteOnly from "../WriteOnly";

interface EndpointTableProps {
  subsystemId: number;
  upstream: Upstream;
  /** Live state; absent until the first status arrives (or if it failed). */
  status: UpstreamStatus | undefined;
}

type Pending = { endpoint: Endpoint; state: "draining" | "disabled" };

const EXPLANATION: Record<Pending["state"], string> = {
  draining:
    "It stops receiving new requests and the requests already on it finish. Use this before taking a server down; it shows as drained when nothing is left on it.",
  disabled:
    "It stops receiving new requests and the requests already on it finish. It stays configured, parked, until you enable it again.",
};

/** The endpoints of one upstream group, with live state and the actions on them. */
export default function EndpointTable({ subsystemId, upstream, status }: EndpointTableProps) {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [editing, setEditing] = useState<Endpoint | null>(null);
  const [removing, setRemoving] = useState<Endpoint | null>(null);
  const [changing, setChanging] = useState<Pending | null>(null);
  const [busy, setBusy] = useState(false);
  const statusOf = (endpoint: Endpoint) => status?.endpoints.find((e) => e.id === endpoint.id);
  const onlyOne = upstream.endpoints.length === 1;

  async function save(endpoint: Endpoint, input: EndpointInput) {
    await api.updateEndpoint(subsystemId, upstream.id, endpoint.id, input);
    await refresh();
    push("success", `Updated endpoint ${input.addr}`);
    setEditing(null);
  }

  /** Runs a call, then refreshes and reports; failures become toasts. */
  async function run(call: () => Promise<unknown>, success: string, done: () => void) {
    setBusy(true);
    try {
      await call();
      await refresh();
      push("success", success);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
      done();
    }
  }

  const setState = (endpoint: Endpoint, state: EndpointState, done: () => void) =>
    run(
      () => api.setEndpointState(subsystemId, upstream.id, endpoint.id, state),
      `Endpoint ${endpoint.addr} is now ${state}`,
      done,
    );

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-[0.88rem]">
        <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
          <tr>
            <th className="py-2 pr-4 font-medium">Address</th>
            <th className="py-2 pr-4 font-medium">Weight</th>
            <th className="py-2 pr-4 font-medium">State</th>
            <th className="py-2 pr-4 text-right font-medium">In flight</th>
            <th className="py-2 pr-4 font-medium">Last check</th>
            <th className="py-2" />
          </tr>
        </thead>
        <tbody>
          {upstream.endpoints.map((endpoint) => {
            const live = statusOf(endpoint);
            return (
              <tr key={endpoint.id} className="border-t border-line">
                <td className="py-2 pr-4 font-mono text-[0.82rem]">{endpoint.addr}</td>
                <td className="py-2 pr-4">{endpoint.weight}</td>
                <td className="py-2 pr-4">
                  {live ? <EndpointStateBadge status={live} /> : <span className="text-ink-faint">{endpoint.state}</span>}
                </td>
                <td className="py-2 pr-4 text-right font-mono">{live ? live.inFlight : "—"}</td>
                <td className="py-2 pr-4 text-ink-muted">
                  {live?.lastProbe
                    ? `${live.lastProbe.ok ? "passed" : "failed"} ${formatDurationMs(live.lastProbe.agoMs)} ago`
                    : upstream.health.check === "none"
                      ? "no checks"
                      : "—"}
                </td>
                <td className="py-2 text-right whitespace-nowrap">
                  <WriteOnly>
                  {endpoint.state === "enabled" ? (
                    <>
                      <button className="btn btn-secondary btn-sm mr-1" onClick={() => setChanging({ endpoint, state: "draining" })} aria-label={`Drain endpoint ${endpoint.addr}`}>
                        Drain
                      </button>
                      <button className="btn btn-secondary btn-sm mr-1" onClick={() => setChanging({ endpoint, state: "disabled" })} aria-label={`Disable endpoint ${endpoint.addr}`}>
                        Disable
                      </button>
                    </>
                  ) : (
                    <button className="btn btn-secondary btn-sm mr-1" disabled={busy} onClick={() => void setState(endpoint, "enabled", () => undefined)} aria-label={`Enable endpoint ${endpoint.addr}`}>
                      Enable
                    </button>
                  )}
                  <button className="icon-btn" onClick={() => setEditing(endpoint)} aria-label={`Edit endpoint ${endpoint.addr}`}>
                    <IconEdit size={16} />
                  </button>
                  <button
                    className="icon-btn danger"
                    onClick={() => setRemoving(endpoint)}
                    disabled={onlyOne}
                    title={onlyOne ? "A group needs at least one endpoint; delete the upstream instead" : undefined}
                    aria-label={`Remove endpoint ${endpoint.addr}`}
                  >
                    <IconTrash size={16} />
                  </button>
                  </WriteOnly>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {editing ? (
        <Modal title={`Edit endpoint ${editing.addr}`} onClose={() => setEditing(null)}>
          <EndpointForm
            initial={editing}
            submitLabel="Save endpoint"
            onSubmit={(input) => save(editing, input)}
            onCancel={() => setEditing(null)}
          />
        </Modal>
      ) : null}

      {changing ? (
        <ConfirmDialog
          title={changing.state === "draining" ? "Drain endpoint?" : "Disable endpoint?"}
          confirmLabel={changing.state === "draining" ? "Drain endpoint" : "Disable endpoint"}
          busy={busy}
          onCancel={() => setChanging(null)}
          onConfirm={() => void setState(changing.endpoint, changing.state, () => setChanging(null))}
        >
          <p>
            <b>{changing.endpoint.addr}</b> of {upstream.name}. {EXPLANATION[changing.state]}
            {statusOf(changing.endpoint)
              ? ` ${statusOf(changing.endpoint)?.inFlight ?? 0} request(s) are on it now.`
              : ""}
          </p>
        </ConfirmDialog>
      ) : null}

      {removing ? (
        <ConfirmDialog
          title="Remove endpoint?"
          confirmLabel="Remove endpoint"
          busy={busy}
          onCancel={() => setRemoving(null)}
          onConfirm={() =>
            void run(
              () => api.removeEndpoint(subsystemId, upstream.id, removing.id),
              `Removed endpoint ${removing.addr}`,
              () => setRemoving(null),
            )
          }
        >
          <p>
            This removes <b>{removing.addr}</b> from {upstream.name}. Requests already on it finish;
            it receives no new ones. You can undo this from the revision history.
          </p>
        </ConfirmDialog>
      ) : null}
    </div>
  );
}
