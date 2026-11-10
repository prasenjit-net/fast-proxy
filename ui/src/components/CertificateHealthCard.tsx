import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { api } from "../lib/api";
import { daysLeft, EXPIRING_DAYS, expiryLabel } from "../lib/certificates";
import Badge from "./Badge";

const REFRESH_MS = 30_000;

/** The soonest certificate expiry, warnings, and whether TLS is on at all. */
export default function CertificateHealthCard() {
  const status = useQuery({ queryKey: ["status"], queryFn: () => api.status(), refetchInterval: REFRESH_MS });
  const certificates = useQuery({
    queryKey: ["certificates"],
    queryFn: () => api.certificates(),
    refetchInterval: REFRESH_MS,
  });
  const tls = status.data?.data.tls;
  const list = certificates.data?.data ?? [];
  const soonest = tls?.soonestExpiry ?? null;
  const days = soonest ? daysLeft(soonest.secondsLeft) : null;
  const onlyDevelopment = tls?.enabled === true && list.length > 0 && list.every((c) => c.source === "self_signed");

  let badge: { tone: "ok" | "warn" | "err" | "neutral"; text: string };
  if (!tls) badge = { tone: "neutral", text: "…" };
  else if (tls.vault === "key mismatch" || tls.unreadable > 0) badge = { tone: "err", text: "needs attention" };
  else if (days !== null && days < 0) badge = { tone: "err", text: "expired" };
  else if (days !== null && days < EXPIRING_DAYS) badge = { tone: "warn", text: "expiring soon" };
  else if (!tls.enabled) badge = { tone: "neutral", text: "TLS off" };
  else badge = { tone: "ok", text: "healthy" };

  return (
    <section className="card">
      <div className="card-head">
        <h2>Certificates</h2>
        <span className="card-hint">
          <Link to="/certificates" className="text-accent hover:underline">
            manage
          </Link>
        </span>
        <span className="ml-auto">
          <Badge tone={badge.tone} dot>
            {badge.text}
          </Badge>
        </span>
      </div>
      {!tls ? (
        <p className="text-[0.88rem] text-ink-faint">{status.isError ? "Certificate status is unavailable." : "Waiting for data…"}</p>
      ) : (
        <div className="flex flex-col gap-2 text-[0.88rem]">
          {!tls.enabled ? (
            <p className="text-ink-muted">
              TLS is off, so no HTTPS listener runs. Start fast-proxy with{" "}
              <span className="font-mono">--tls</span> to serve certificates.
            </p>
          ) : null}
          {tls.vault === "key mismatch" ? (
            <p role="alert" className="text-err">
              The master key does not match this database: stored private keys cannot be read.
            </p>
          ) : null}
          {tls.unreadable > 0 && tls.vault !== "key mismatch" ? (
            <p role="alert" className="text-err">
              {tls.unreadable} certificate{tls.unreadable === 1 ? "" : "s"} cannot be read.
            </p>
          ) : null}
          {soonest ? (
            <p className={days !== null && days < EXPIRING_DAYS ? "text-warn" : "text-ink-muted"}>
              {days !== null && days < 0 ? "Expired: " : "Soonest expiry: "}
              <b>{soonest.name}</b> {expiryLabel(soonest.secondsLeft)}
            </p>
          ) : (
            <p className="text-ink-faint">No certificates yet.</p>
          )}
          {onlyDevelopment ? (
            <p className="text-warn">Only development (self-signed) certificates exist; browsers will warn visitors.</p>
          ) : null}
        </div>
      )}
    </section>
  );
}
