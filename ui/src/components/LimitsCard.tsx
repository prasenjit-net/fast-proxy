// The limits panel of the dashboard (ADR 0029): open connections against their
// cap, how full the rate-limit key table is, and what the limits refused, by
// reason. Everything here is this instance's own.
import { useQuery } from "@tanstack/react-query";
import { useLive } from "../context/LiveContext";
import { api } from "../lib/api";
import { formatNumber } from "../lib/format";
import type { ReasonCount } from "../lib/types";
import Badge from "./Badge";

const REFRESH_MS = 10_000;
/** The key table is worth a warning from this share on. */
const KEYS_WARN = 0.8;

/** How each reason reads on the panel; an unknown one shows as it is. */
const REASON_LABELS: Record<string, string> = {
  rate_limited: "over a rate limit (429)",
  payload_too_large: "body too large (413)",
  uri_too_long: "target too long (414)",
  request_timeout: "body too slow (408)",
  endpoint_capacity: "endpoints full (503)",
  max_connections: "connection cap",
  max_connections_per_client: "per-client cap",
  first_byte: "sent nothing",
  send: "stopped reading",
  body: "body stalled",
  tunnel_idle: "idle tunnel",
};

function Reasons({ title, counts }: { title: string; counts: ReasonCount[] }) {
  const listed = counts.filter((entry) => entry.count > 0);
  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-[0.82rem] font-medium text-ink-muted">{title}</h3>
      {listed.length === 0 ? (
        <p className="text-[0.85rem] text-ink-faint">none</p>
      ) : (
        <ul aria-label={title} className="flex flex-col gap-0.5 text-[0.85rem]">
          {listed.map((entry) => (
            <li key={entry.reason} className="flex justify-between gap-3">
              <span>{REASON_LABELS[entry.reason] ?? entry.reason}</span>
              <span className="font-mono">{formatNumber(entry.count)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** This instance's limits and their refusals. */
export default function LimitsCard() {
  const { traffic } = useLive();
  const query = useQuery({ queryKey: ["limits"], queryFn: () => api.limits(), refetchInterval: REFRESH_MS });
  const limits = query.data?.data;

  let badge: { tone: "ok" | "warn" | "neutral"; text: string } = { tone: "neutral", text: "…" };
  if (limits) {
    const { used, capacity } = limits.rateLimitKeys;
    const full = limits.rateLimitTableFull > 0 || (capacity > 0 && used / capacity >= KEYS_WARN);
    badge = full ? { tone: "warn", text: "key table filling" } : { tone: "ok", text: "within limits" };
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Limits</h2>
        <span className="card-hint">this instance; set in [limits] and per route</span>
        <span className="ml-auto">
          <Badge tone={badge.tone} dot>
            {badge.text}
          </Badge>
        </span>
      </div>
      {!limits ? (
        <p className="text-[0.88rem] text-ink-faint">
          {query.isError ? "The limits are unavailable." : "Waiting for data…"}
        </p>
      ) : (
        <div className="flex flex-col gap-4">
          <dl className="grid grid-cols-1 gap-3 text-[0.88rem] sm:grid-cols-3">
            <div>
              <dt className="text-ink-muted">Connections</dt>
              <dd className="font-mono">
                {traffic ? formatNumber(traffic.activeConnections) : "…"} of{" "}
                {formatNumber(limits.settings.maxConnections)}
              </dd>
              <dd className="text-[0.8rem] text-ink-faint">
                {limits.settings.maxConnectionsPerClient === null
                  ? "no cap per client"
                  : `at most ${formatNumber(limits.settings.maxConnectionsPerClient)} per client`}
              </dd>
            </div>
            <div>
              <dt className="text-ink-muted">Rate-limit keys</dt>
              <dd className="font-mono">
                {formatNumber(limits.rateLimitKeys.used)} of {formatNumber(limits.rateLimitKeys.capacity)}
              </dd>
              {limits.rateLimitTableFull > 0 ? (
                <dd className="text-[0.8rem] text-warn">
                  {formatNumber(limits.rateLimitTableFull)} new clients let through while full
                </dd>
              ) : null}
            </div>
            <div>
              <dt className="text-ink-muted">Request body</dt>
              <dd className="font-mono">
                {limits.settings.maxBodyBytes === null
                  ? "unlimited"
                  : `${formatNumber(limits.settings.maxBodyBytes)} bytes`}
              </dd>
              <dd className="text-[0.8rem] text-ink-faint">unless a route sets its own</dd>
            </div>
          </dl>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
            <Reasons title="Requests refused" counts={limits.requestsRefused} />
            <Reasons title="Connections refused" counts={limits.connectionsRefused} />
            <Reasons title="Clients timed out" counts={limits.clientTimeouts} />
          </div>
        </div>
      )}
    </section>
  );
}
