import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { IconX } from "../../icons";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { Host, SubsystemDetail } from "../../lib/types";
import ConfirmDialog from "../ConfirmDialog";
import HostForm from "../forms/HostForm";

/** The host names a subsystem answers to: add and remove. */
export default function HostsSection({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [removing, setRemoving] = useState<Host | null>(null);
  const [busy, setBusy] = useState(false);

  async function add(pattern: string) {
    await api.addHost(subsystem.id, pattern);
    await refresh();
    push("success", `${pattern} now routes to ${subsystem.name}`);
  }

  async function remove(host: Host) {
    setBusy(true);
    try {
      await api.removeHost(subsystem.id, host.id);
      await refresh();
      push("success", `Removed ${host.pattern}`);
      setRemoving(null);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Hosts</h2>
        <span className="card-hint">requests are matched to a subsystem by their Host header</span>
      </div>
      {subsystem.hosts.length === 0 ? (
        <p className="mb-4 text-[0.88rem] text-ink-muted">
          No hosts yet, so this subsystem receives no traffic. Add the host name clients will use.
        </p>
      ) : (
        <ul className="mb-4 flex flex-wrap gap-2">
          {subsystem.hosts.map((host) => (
            <li
              key={host.id}
              className="inline-flex items-center gap-1 rounded-full bg-surface-2 py-1 pr-1 pl-3 font-mono text-[0.8rem]"
            >
              {host.pattern}
              <button
                className="icon-btn danger size-6"
                onClick={() => setRemoving(host)}
                aria-label={`Remove ${host.pattern}`}
              >
                <IconX size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <HostForm onSubmit={add} />
      {removing ? (
        <ConfirmDialog
          title="Remove host?"
          confirmLabel="Remove host"
          busy={busy}
          onCancel={() => setRemoving(null)}
          onConfirm={() => void remove(removing)}
        >
          <p>
            Requests for <b className="font-mono">{removing.pattern}</b> will stop reaching{" "}
            <b>{subsystem.name}</b> and get a 404 unless another subsystem answers to it.
          </p>
        </ConfirmDialog>
      ) : null}
    </section>
  );
}
