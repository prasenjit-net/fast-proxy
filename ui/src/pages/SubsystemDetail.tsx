import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import Badge from "../components/Badge";
import ConfirmDialog from "../components/ConfirmDialog";
import HostsSection from "../components/subsystem/HostsSection";
import HttpsSection from "../components/subsystem/HttpsSection";
import MaintenanceSection from "../components/subsystem/MaintenanceSection";
import RoutesSection from "../components/subsystem/RoutesSection";
import SubsystemSettings from "../components/subsystem/SubsystemSettings";
import SamplingControl from "../components/sampling/SamplingControl";
import UpstreamsSection from "../components/subsystem/UpstreamsSection";
import WriteOnly from "../components/WriteOnly";
import { useToast } from "../context/ToastContext";
import { IconTrash } from "../icons";
import { api, ApiError } from "../lib/api";
import { refreshConfigData } from "../lib/refreshConfigData";
import { queryClient } from "../lib/queryClient";

export default function SubsystemDetailPage() {
  const { id: rawId } = useParams({ strict: false }) as { id?: string };
  const id = Number(rawId);
  const navigate = useNavigate();
  const { push, notifyError } = useToast();
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  const query = useQuery({
    queryKey: ["subsystem", id],
    queryFn: () => api.subsystem(id),
    enabled: Number.isInteger(id),
  });

  const remove = useMutation({
    mutationFn: () => api.deleteSubsystem(id),
    onSuccess: async () => {
      await refreshConfigData(queryClient);
      push("success", "Subsystem deleted");
      await navigate({ to: "/subsystems" });
    },
    onError: (error) => {
      setConfirmingDelete(false);
      notifyError(error);
    },
  });

  if (query.isError) {
    const missing = query.error instanceof ApiError && query.error.status === 404;
    return (
      <div className="card flex flex-col items-start gap-3">
        <h2 className="text-[1rem] font-semibold">
          {missing ? "This subsystem does not exist" : "Could not load the subsystem"}
        </h2>
        <p className="text-[0.9rem] text-ink-muted">
          {missing ? "It may have been deleted." : String(query.error.message)}
        </p>
        <Link to="/subsystems" className="btn btn-secondary btn-sm">
          Back to subsystems
        </Link>
      </div>
    );
  }
  if (!query.data) {
    return (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  }

  const subsystem = query.data.data;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <Link to="/subsystems" className="text-[0.85rem] text-ink-muted hover:text-ink">
          ← Subsystems
        </Link>
        <h2 className="text-[1.15rem] font-semibold">{subsystem.name}</h2>
        <Badge tone={subsystem.enabled ? "ok" : "warn"} dot>
          {subsystem.enabled ? "enabled" : "disabled"}
        </Badge>
        <WriteOnly>
          <button className="btn btn-danger btn-sm ml-auto" onClick={() => setConfirmingDelete(true)}>
            <IconTrash size={14} /> Delete subsystem
          </button>
        </WriteOnly>
      </div>

      <div className="card-grid">
        <SubsystemSettings subsystem={subsystem} />
        <div className="flex flex-col gap-4">
          <MaintenanceSection subsystem={subsystem} />
          <section className="card">
            <div className="card-head">
              <h2>Request sampling</h2>
              <span className="card-hint">routes can differ; they inherit from here</span>
            </div>
            <SamplingControl target={{ subsystemId: subsystem.id }} label={subsystem.name} />
          </section>
        </div>
      </div>
      <div className="card-grid">
        <HostsSection subsystem={subsystem} />
        <HttpsSection subsystem={subsystem} />
      </div>
      <UpstreamsSection subsystem={subsystem} />
      <RoutesSection subsystem={subsystem} />

      {confirmingDelete ? (
        <ConfirmDialog
          title="Delete subsystem?"
          confirmLabel="Delete subsystem"
          busy={remove.isPending}
          onCancel={() => setConfirmingDelete(false)}
          onConfirm={() => remove.mutate()}
        >
          <p>
            This removes <b>{subsystem.name}</b> with its {subsystem.hosts.length} host(s),{" "}
            {subsystem.upstreams.length} upstream(s) and {subsystem.routes.length} route(s). Its hosts
            will get a 404. You can undo this from the revision history.
          </p>
        </ConfirmDialog>
      ) : null}
    </div>
  );
}
