import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import Badge from "../components/Badge";
import EmptyState from "../components/EmptyState";
import { api } from "../lib/api";
import { LB_ALGORITHMS } from "../lib/policyValidation";
import type { LbAlgorithm, MaintenanceScope, SubsystemDetail, Upstream } from "../lib/types";
import { stillActive, useMaintenance } from "../lib/useMaintenance";
import { useNow } from "../lib/useNow";
import { useUpstreamStatus } from "../lib/useUpstreamStatus";

function algorithmLabel(algorithm: LbAlgorithm): string {
  return LB_ALGORITHMS.find((choice) => choice.value === algorithm)?.label ?? algorithm;
}

/** Whether the route, its subsystem or the whole proxy is in maintenance. */
function maintenanceOf(
  active: MaintenanceScope[],
  subsystemId: number,
  routeId: number | null,
): MaintenanceScope | undefined {
  return (
    active.find((scope) => scope.scope === "route" && routeId !== null && scope.routeId === routeId) ??
    active.find((scope) => scope.scope === "subsystem" && scope.subsystemId === subsystemId) ??
    active.find((scope) => scope.scope === "global")
  );
}

function MaintenanceBadge({ scope }: { scope: MaintenanceScope | undefined }) {
  if (!scope) return <span className="text-ink-faint">—</span>;
  const via = scope.scope === "route" ? "route" : scope.scope === "subsystem" ? "subsystem" : "whole proxy";
  return (
    <Badge tone="warn" dot>
      maintenance ({via})
    </Badge>
  );
}

/** Healthy endpoints of one group, from its live status. */
function HealthBadge({ subsystemId, upstream }: { subsystemId: number; upstream: Upstream }) {
  const status = useUpstreamStatus(subsystemId, upstream.id);
  if (!status.data) return <span className="text-ink-faint">…</span>;
  const { healthy, endpoints } = status.data.data;
  const tone = healthy === endpoints.length ? "ok" : healthy === 0 ? "err" : "warn";
  return (
    <Badge tone={tone} dot>
      {healthy}/{endpoints.length} healthy
    </Badge>
  );
}

function SubsystemLink({ subsystem }: { subsystem: SubsystemDetail }) {
  return (
    <Link to="/subsystems/$id" params={{ id: String(subsystem.id) }} className="underline">
      {subsystem.name}
    </Link>
  );
}

const HEAD = "py-2 pr-4 font-medium";

/** Every route and upstream group of every subsystem in one read-only view. */
export default function RoutesOverviewPage() {
  const summaries = useQuery({ queryKey: ["subsystems"], queryFn: api.subsystems });
  const maintenance = useMaintenance();
  const now = useNow();
  const details = useQueries({
    queries: (summaries.data?.data ?? []).map((summary) => ({
      queryKey: ["subsystem", summary.id],
      queryFn: () => api.subsystem(summary.id),
    })),
  });

  if (summaries.isError) {
    return <p className="text-[0.9rem] text-err">Could not load subsystems: {summaries.error.message}</p>;
  }
  if (!summaries.data || details.some((detail) => !detail.data)) {
    return (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  }
  const subsystems = details.flatMap((detail) => (detail.data ? [detail.data.data] : []));
  if (subsystems.length === 0) {
    return (
      <section className="card">
        <EmptyState title="No subsystems yet">Create a subsystem to give the proxy something to route.</EmptyState>
      </section>
    );
  }
  const active = (maintenance.data?.data ?? []).filter((scope) => stillActive(scope, now));

  return (
    <div className="flex flex-col gap-4">
      <section className="card">
        <div className="card-head">
          <h2>Routes</h2>
          <span className="card-hint">in match order within each subsystem</span>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[0.88rem]">
            <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
              <tr>
                <th className={HEAD}>Subsystem</th>
                <th className={HEAD}>Prefix</th>
                <th className={HEAD}>Upstream</th>
                <th className={HEAD}>Health</th>
                <th className={HEAD}>Enabled</th>
                <th className="py-2 font-medium">Maintenance</th>
              </tr>
            </thead>
            <tbody>
              {subsystems.flatMap((subsystem) =>
                subsystem.routes.map((route) => {
                  const group = subsystem.upstreams.find((item) => item.id === route.upstreamGroupId);
                  return (
                    <tr key={`${subsystem.id}-${route.id}`} className="border-t border-line">
                      <td className="py-2 pr-4">
                        <SubsystemLink subsystem={subsystem} />
                      </td>
                      <td className="py-2 pr-4 font-mono text-[0.82rem]">{route.prefix}</td>
                      <td className="py-2 pr-4">{group?.name ?? `#${route.upstreamGroupId}`}</td>
                      <td className="py-2 pr-4">
                        {group ? <HealthBadge subsystemId={subsystem.id} upstream={group} /> : "—"}
                      </td>
                      <td className="py-2 pr-4">{route.enabled && subsystem.enabled ? "On" : "Off"}</td>
                      <td className="py-2">
                        <MaintenanceBadge scope={maintenanceOf(active, subsystem.id, route.id)} />
                      </td>
                    </tr>
                  );
                }),
              )}
            </tbody>
          </table>
        </div>
      </section>

      <section className="card">
        <div className="card-head">
          <h2>Upstream groups</h2>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[0.88rem]">
            <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
              <tr>
                <th className={HEAD}>Subsystem</th>
                <th className={HEAD}>Group</th>
                <th className={HEAD}>Algorithm</th>
                <th className={HEAD}>Endpoints</th>
                <th className={HEAD}>Health</th>
                <th className="py-2 font-medium">Maintenance</th>
              </tr>
            </thead>
            <tbody>
              {subsystems.flatMap((subsystem) =>
                subsystem.upstreams.map((group) => (
                  <tr key={`${subsystem.id}-${group.id}`} className="border-t border-line">
                    <td className="py-2 pr-4">
                      <SubsystemLink subsystem={subsystem} />
                    </td>
                    <td className="py-2 pr-4 font-medium">{group.name}</td>
                    <td className="py-2 pr-4 text-ink-muted">{algorithmLabel(group.lb.algorithm)}</td>
                    <td className="py-2 pr-4">{group.endpoints.length}</td>
                    <td className="py-2 pr-4">
                      <HealthBadge subsystemId={subsystem.id} upstream={group} />
                    </td>
                    <td className="py-2">
                      <MaintenanceBadge scope={maintenanceOf(active, subsystem.id, null)} />
                    </td>
                  </tr>
                )),
              )}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}
