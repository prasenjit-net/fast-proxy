import { useQuery } from "@tanstack/react-query";
import { api } from "../lib/api";
import Badge from "./Badge";

const REFRESH_MS = 10_000;

/** How many endpoints are in rotation, and which are not and why. */
export default function UpstreamHealthCard() {
  const status = useQuery({
    queryKey: ["status"],
    queryFn: () => api.status(),
    refetchInterval: REFRESH_MS,
  });
  const health = status.data?.data.upstreams;

  return (
    <section className="card">
      <div className="card-head">
        <h2>Upstream health</h2>
        <span className="card-hint">endpoints taking requests</span>
        {health && health.total > 0 ? (
          <span className="ml-auto">
            <Badge tone={health.healthy === health.total ? "ok" : health.healthy === 0 ? "err" : "warn"} dot>
              {health.healthy} of {health.total} healthy
            </Badge>
          </span>
        ) : null}
      </div>
      {!health ? (
        <p className="text-[0.88rem] text-ink-faint">{status.isError ? "Status is unavailable." : "Waiting for data…"}</p>
      ) : health.total === 0 ? (
        <p className="text-[0.88rem] text-ink-faint">No endpoints are configured yet.</p>
      ) : health.unhealthy.length === 0 ? (
        <p className="text-[0.88rem] text-ink-muted">Every endpoint is taking requests.</p>
      ) : (
        <ul className="m-0 flex list-none flex-col gap-1 p-0 text-[0.88rem]">
          {health.unhealthy.map((entry) => (
            <li key={`${entry.subsystem}/${entry.upstream}/${entry.endpoint}`} className="flex flex-wrap gap-x-3 border-t border-line py-1.5 first:border-t-0">
              <span className="font-mono text-[0.82rem]">{entry.endpoint}</span>
              <span className="text-ink-muted">
                {entry.subsystem} / {entry.upstream}
              </span>
              <span className="text-warn">{entry.reason}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
