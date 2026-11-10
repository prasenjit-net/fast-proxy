import { formatDurationMs } from "../lib/format";
import type { SubsystemView } from "../lib/types";
import Badge from "./Badge";

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
            <dt className="text-ink-muted">Upstreams</dt>
            <dd className="font-mono">
              {subsystem.upstreams
                .map((u) => `${u.name} → ${u.endpoints.map((e) => e.addr).join(" ")}`)
                .join(", ") || "none"}
            </dd>
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
