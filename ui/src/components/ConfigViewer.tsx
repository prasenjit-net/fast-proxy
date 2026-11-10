import { formatDurationMs } from "../lib/format";
import { LB_ALGORITHMS } from "../lib/policyValidation";
import type { SubsystemView, Upstream } from "../lib/types";
import Badge from "./Badge";

function httpsSummary(https: SubsystemView["https"]): string {
  if (!https.enabled) return "off";
  const parts = ["on"];
  if (https.redirect !== "off") parts.push(`redirect ${https.redirect}`);
  if (https.hsts.enabled) {
    parts.push(`HSTS ${https.hsts.maxAgeSecs}s${https.hsts.includeSubdomains ? " + subdomains" : ""}`);
  }
  parts.push(https.certificateId === null ? "no certificate" : `certificate #${https.certificateId}`);
  return parts.join(", ");
}

/** A group's policy in one line: how it balances, checks health and talks TLS. */
function policySummary(upstream: Upstream): string {
  const algorithm = LB_ALGORITHMS.find((entry) => entry.value === upstream.lb.algorithm)?.label ?? upstream.lb.algorithm;
  const parts = [algorithm, upstream.failurePolicy.replace("_", " ")];
  if (upstream.health.check !== "none") parts.push(`${upstream.health.check} health check`);
  if (upstream.passive.consecutiveFailures > 0) parts.push(`eject after ${upstream.passive.consecutiveFailures} failures`);
  if (upstream.tls.enabled) {
    const detail = [upstream.tls.verify ? null : "not verified", upstream.tls.ca ? `CA ${upstream.tls.ca}` : null]
      .filter(Boolean)
      .join(", ");
    parts.push(detail ? `TLS (${detail})` : "TLS");
  }
  return parts.join(" · ");
}

/** A read-only summary of a configuration snapshot (for example a revision). */
export default function ConfigViewer({ subsystems }: { subsystems: SubsystemView[] }) {
  if (subsystems.length === 0) {
    return <p className="text-[0.88rem] text-ink-muted">This revision has no subsystems.</p>;
  }
  return (
    <div className="flex flex-col gap-4">
      {subsystems.map((subsystem) => (
        <section key={subsystem.id} className="rounded-lg border border-line p-3">
          <div className="mb-2 flex items-center gap-2">
            <h3 className="font-semibold">{subsystem.name}</h3>
            <Badge tone={subsystem.enabled ? "ok" : "warn"} dot>
              {subsystem.enabled ? "enabled" : "disabled"}
            </Badge>
          </div>
          <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-[0.84rem]">
            <dt className="text-ink-muted">Hosts</dt>
            <dd className="font-mono">{subsystem.hosts.join(", ") || "none"}</dd>
            <dt className="text-ink-muted">HTTPS</dt>
            <dd>{httpsSummary(subsystem.https)}</dd>
            <dt className="text-ink-muted">Upstreams</dt>
            <dd className="font-mono">
              {subsystem.upstreams
                .map((u) => `${u.name} → ${u.endpoints.map((e) => e.addr).join(" ")}`)
                .join(", ") || "none"}
            </dd>
            {subsystem.upstreams.length > 0 ? (
              <>
                <dt className="text-ink-muted">Policies</dt>
                <dd>
                  {subsystem.upstreams.map((u) => (
                    <div key={u.id}>
                      <span className="font-mono">{u.name}</span>
                      <span className="text-ink-faint"> {policySummary(u)}</span>
                    </div>
                  ))}
                </dd>
              </>
            ) : null}
            <dt className="text-ink-muted">Routes</dt>
            <dd>
              {subsystem.routes.length === 0
                ? "none"
                : subsystem.routes.map((route) => (
                    <div key={route.id} className="font-mono">
                      {route.prefix} →{" "}
                      {subsystem.upstreams.find((u) => u.id === route.upstreamGroupId)?.name ??
                        `#${route.upstreamGroupId}`}
                      <span className="text-ink-faint">
                        {" "}
                        ({formatDurationMs(route.connectTimeoutMs)}/{formatDurationMs(route.responseTimeoutMs)}
                        {route.enabled ? "" : ", disabled"})
                      </span>
                    </div>
                  ))}
            </dd>
          </dl>
        </section>
      ))}
    </div>
  );
}
