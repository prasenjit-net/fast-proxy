import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { acmeStateBadge, isFailing, relativeTime } from "../../lib/acme";
import { api } from "../../lib/api";
import { useNow } from "../../lib/useNow";
import Badge from "../Badge";

/** ACME for the whole proxy: the CA, the account, who renews, and every subsystem's status. */
export default function AcmeOverviewSection() {
  const [onlyFailing, setOnlyFailing] = useState(false);
  const now = useNow(30_000);
  const acme = useQuery({ queryKey: ["acme"], queryFn: () => api.acme() });
  const data = acme.data?.data;
  if (!data) return null;
  const shown = data.subsystems.filter((status) => !onlyFailing || isFailing(status));

  return (
    <section className="card" aria-label="ACME">
      <div className="card-head">
        <h2>Automatic certificates (ACME)</h2>
        <span className="ml-auto">
          {data.enabled ? <Badge tone="ok">on</Badge> : <Badge>off</Badge>}
        </span>
      </div>
      {data.enabled ? (
        <>
          <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-[0.88rem]">
            <dt className="text-ink-muted">Directory</dt>
            <dd className="break-all font-mono">
              {data.directory}{" "}
              {data.staging ? <Badge tone="warn">staging: not publicly trusted</Badge> : null}
            </dd>
            <dt className="text-ink-muted">Allowed domains</dt>
            <dd className="font-mono">{data.allowedDomains.join(", ")}</dd>
            <dt className="text-ink-muted">Account</dt>
            <dd>{data.accountRegistered ? "registered" : "created on the first order"}</dd>
            <dt className="text-ink-muted">Renewing instance</dt>
            <dd>
              {data.lease
                ? `${data.lease.holder}${data.lease.heldHere ? " (this one)" : ""}, lease expires ${relativeTime(data.lease.expiresAtMs, now)}`
                : "none yet"}
            </dd>
          </dl>
          <div className="mt-4 flex items-center gap-2">
            <label className="flex items-center gap-2 text-[0.86rem]">
              <input
                type="checkbox"
                checked={onlyFailing}
                onChange={(event) => setOnlyFailing(event.target.checked)}
              />
              Only failing
            </label>
          </div>
          {shown.length === 0 ? (
            <p className="mt-2 text-[0.86rem] text-ink-muted">
              {data.subsystems.length === 0 ? "No subsystem uses ACME yet." : "Nothing is failing."}
            </p>
          ) : (
            <div className="mt-2 overflow-x-auto">
              <table className="w-full text-left text-[0.88rem]">
                <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
                  <tr>
                    <th className="py-2 pr-4 font-medium">Subsystem</th>
                    <th className="py-2 pr-4 font-medium">State</th>
                    <th className="py-2 pr-4 font-medium">Names</th>
                    <th className="py-2 pr-4 font-medium">Expires</th>
                    <th className="py-2 pr-4 font-medium">Next attempt</th>
                    <th className="py-2 font-medium">Last error</th>
                  </tr>
                </thead>
                <tbody>
                  {shown.map((status) => {
                    const badge = acmeStateBadge(status.state);
                    return (
                      <tr key={status.subsystemId} className="border-t border-line align-top">
                        <td className="py-2 pr-4 font-medium">{status.subsystem}</td>
                        <td className="py-2 pr-4">
                          <Badge tone={badge.tone} dot>
                            {badge.label}
                          </Badge>
                        </td>
                        <td className="py-2 pr-4 font-mono text-[0.8rem]">{status.eligibleNames.join(", ")}</td>
                        <td className="py-2 pr-4">
                          {status.notAfterMs === null ? "—" : relativeTime(status.notAfterMs, now)}
                        </td>
                        <td className="py-2 pr-4">
                          {status.nextAttemptAtMs === null ? "—" : relativeTime(status.nextAttemptAtMs, now)}
                        </td>
                        <td className="py-2 text-err">{status.lastError}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </>
      ) : (
        <p className="text-[0.86rem] text-ink-muted">
          ACME is off for this proxy. Turn it on under <span className="font-mono">[data_plane.acme]</span> in the
          config file to have certificates issued and renewed automatically.
        </p>
      )}
    </section>
  );
}
