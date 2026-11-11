import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import Badge from "../components/Badge";
import EmptyState from "../components/EmptyState";
import SubsystemForm from "../components/forms/SubsystemForm";
import Modal from "../components/Modal";
import Toggle from "../components/Toggle";
import WriteOnly from "../components/WriteOnly";
import { useCanWrite } from "../context/SessionContext";
import { useToast } from "../context/ToastContext";
import { IconPlus } from "../icons";
import { api } from "../lib/api";
import { timeAgo } from "../lib/format";
import { queryClient } from "../lib/queryClient";
import { refreshConfigData } from "../lib/refreshConfigData";
import type { SubsystemInput, SubsystemSummary } from "../lib/types";

const MAX_HOST_CHIPS = 3;

function HostChips({ hosts }: { hosts: string[] }) {
  if (hosts.length === 0) return <span className="text-ink-faint">no hosts</span>;
  return (
    <span className="flex flex-wrap gap-1.5">
      {hosts.slice(0, MAX_HOST_CHIPS).map((host) => (
        <Badge key={host}>{host}</Badge>
      ))}
      {hosts.length > MAX_HOST_CHIPS ? <Badge>+{hosts.length - MAX_HOST_CHIPS} more</Badge> : null}
    </span>
  );
}

export default function SubsystemsPage() {
  const navigate = useNavigate();
  const { push, notifyError } = useToast();
  const canWrite = useCanWrite();
  const [creating, setCreating] = useState(false);
  const query = useQuery({ queryKey: ["subsystems"], queryFn: api.subsystems });

  const setEnabled = useMutation({
    mutationFn: ({ subsystem, enabled }: { subsystem: SubsystemSummary; enabled: boolean }) =>
      api.updateSubsystem(subsystem.id, {
        name: subsystem.name,
        description: subsystem.description,
        enabled,
      }),
    onSuccess: async (_result, { subsystem, enabled }) => {
      await refreshConfigData(queryClient);
      push("success", `${subsystem.name} ${enabled ? "enabled" : "disabled"}`);
    },
    onError: notifyError,
  });

  async function create(input: SubsystemInput) {
    const created = await api.createSubsystem(input);
    await refreshConfigData(queryClient);
    push("success", `Created ${input.name}`);
    setCreating(false);
    await navigate({ to: "/subsystems/$id", params: { id: String(created.data.id) } });
  }

  const newButton = (
    <WriteOnly>
      <button className="btn btn-primary" onClick={() => setCreating(true)}>
        <IconPlus size={16} /> New subsystem
      </button>
    </WriteOnly>
  );

  let body;
  if (query.isError) {
    body = <p className="text-[0.9rem] text-err">Could not load subsystems: {query.error.message}</p>;
  } else if (!query.data) {
    body = (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  } else if (query.data.data.length === 0) {
    body = (
      <EmptyState title="No subsystems yet" action={newButton}>
        A subsystem is one host name served by the proxy. Create one, add its host name, point an
        upstream at your backend, and add a route to send traffic there.
      </EmptyState>
    );
  } else {
    body = (
      <div className="overflow-x-auto">
        <table className="w-full text-left text-[0.88rem]">
          <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
            <tr>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Hosts</th>
              <th className="py-2 pr-4 font-medium">Routes</th>
              <th className="py-2 pr-4 font-medium">Enabled</th>
              <th className="py-2 font-medium">Updated</th>
            </tr>
          </thead>
          <tbody>
            {query.data.data.map((subsystem) => (
              <tr key={subsystem.id} className="border-t border-line align-middle">
                <td className="py-3 pr-4">
                  <Link
                    to="/subsystems/$id"
                    params={{ id: String(subsystem.id) }}
                    className="font-medium text-accent hover:underline"
                  >
                    {subsystem.name}
                  </Link>
                  {subsystem.description ? (
                    <div className="max-w-[28ch] truncate text-[0.78rem] text-ink-faint">
                      {subsystem.description}
                    </div>
                  ) : null}
                </td>
                <td className="py-3 pr-4">
                  <HostChips hosts={subsystem.hosts} />
                </td>
                <td className="py-3 pr-4 text-ink-muted">{subsystem.routeCount}</td>
                <td className="py-3 pr-4">
                  {canWrite ? (
                    <Toggle
                      checked={subsystem.enabled}
                      disabled={setEnabled.isPending}
                      onChange={(enabled) => setEnabled.mutate({ subsystem, enabled })}
                      label={subsystem.enabled ? "On" : "Off"}
                    />
                  ) : (
                    <Badge tone={subsystem.enabled ? "ok" : "warn"} dot>
                      {subsystem.enabled ? "On" : "Off"}
                    </Badge>
                  )}
                </td>
                <td className="py-3 text-ink-faint">{timeAgo(subsystem.updatedAtMs)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Subsystems</h2>
        <span className="card-hint">one per host name</span>
        {query.data && query.data.data.length > 0 ? <div className="ml-auto">{newButton}</div> : null}
      </div>
      {body}
      {creating ? (
        <Modal title="New subsystem" onClose={() => setCreating(false)}>
          <SubsystemForm
            submitLabel="Create subsystem"
            onSubmit={create}
            onCancel={() => setCreating(false)}
          />
        </Modal>
      ) : null}
    </section>
  );
}
