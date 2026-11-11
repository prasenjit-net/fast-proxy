import { Link } from "@tanstack/react-router";
import Badge from "../components/Badge";
import EmptyState from "../components/EmptyState";
import Countdown from "../components/maintenance/Countdown";
import MaintenanceControl from "../components/maintenance/MaintenanceControl";
import { scopeLabel } from "../components/MaintenanceBanner";
import { timeAgo } from "../lib/format";
import type { MaintenanceScope } from "../lib/types";
import { stillActive, useMaintenance } from "../lib/useMaintenance";
import { useNow } from "../lib/useNow";

/** How a scope lets some requests through, in a few words. */
function bypassSummary(scope: MaintenanceScope): string {
  const parts: string[] = [];
  if (scope.bypass.cidrs.length > 0) parts.push(scope.bypass.cidrs.join(", "));
  if (scope.bypass.header) parts.push(`header ${scope.bypass.header}`);
  return parts.length > 0 ? parts.join(" · ") : "none";
}

function ScopeLink({ scope }: { scope: MaintenanceScope }) {
  if (scope.subsystemId === null) return <>{scopeLabel(scope)}</>;
  return (
    <Link to="/subsystems/$id" params={{ id: String(scope.subsystemId) }} className="underline">
      {scopeLabel(scope)}
    </Link>
  );
}

/** Every scope in maintenance across the proxy, and the control for the whole proxy. */
export default function MaintenancePage() {
  const query = useMaintenance();
  const now = useNow();
  const active = (query.data?.data ?? []).filter((scope) => stillActive(scope, now));

  let body;
  if (query.isError) {
    body = <p className="text-[0.9rem] text-err">Could not load maintenance: {query.error.message}</p>;
  } else if (!query.data) {
    body = (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  } else if (active.length === 0) {
    body = (
      <EmptyState title="Nothing is in maintenance">
        Every host answers normally. Turn maintenance on for the whole proxy here, or for one
        subsystem or route from its page.
      </EmptyState>
    );
  } else {
    body = (
      <div className="overflow-x-auto">
        <table className="w-full text-left text-[0.88rem]">
          <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
            <tr>
              <th className="py-2 pr-4 font-medium">Covers</th>
              <th className="py-2 pr-4 font-medium">Message</th>
              <th className="py-2 pr-4 font-medium">Ends</th>
              <th className="py-2 pr-4 font-medium">Let through</th>
              <th className="py-2 font-medium">Set</th>
            </tr>
          </thead>
          <tbody>
            {active.map((scope) => (
              <tr
                key={`${scope.scope}-${scope.subsystemId}-${scope.routeId}`}
                className="border-t border-line"
              >
                <td className="py-2 pr-4 font-medium">
                  <ScopeLink scope={scope} />
                </td>
                <td className="py-2 pr-4 text-ink-muted">{scope.message || "—"}</td>
                <td className="py-2 pr-4">
                  <Badge tone="warn" dot>
                    <Countdown expiresAtMs={scope.expiresAtMs} />
                  </Badge>
                </td>
                <td className="py-2 pr-4 font-mono text-[0.78rem] text-ink-muted">
                  {bypassSummary(scope)}
                </td>
                <td className="py-2 text-ink-faint" title={new Date(scope.setAtMs).toLocaleString()}>
                  {timeAgo(scope.setAtMs)} by {scope.setBy}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <section className="card">
        <div className="card-head">
          <h2>Whole proxy</h2>
          <span className="card-hint">every host answers 503 while this is on</span>
        </div>
        <MaintenanceControl target={{ scope: "global" }} />
      </section>
      <section className="card">
        <div className="card-head">
          <h2>In maintenance now</h2>
          <span className="card-hint">{active.length} scope(s)</span>
        </div>
        {body}
      </section>
    </div>
  );
}
