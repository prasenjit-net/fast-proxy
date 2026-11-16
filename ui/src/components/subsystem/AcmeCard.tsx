import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useCanWrite } from "../../context/SessionContext";
import { useToast } from "../../context/ToastContext";
import { acmeStateBadge, relativeTime, skipReasonText } from "../../lib/acme";
import { api } from "../../lib/api";
import type { SkippedName, SubsystemDetail } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import Badge from "../Badge";
import Toggle from "../Toggle";
import WriteOnly from "../WriteOnly";

interface Props {
  subsystem: SubsystemDetail;
  /** Saves the ACME switch with the rest of the HTTPS settings; absent in a read-only view. */
  onToggle?: (enabled: boolean) => Promise<void>;
}

function Names({ names }: Readonly<{ names: string[] }>) {
  return names.length === 0 ? (
    <span className="text-ink-muted">none</span>
  ) : (
    <span className="font-mono">{names.join(", ")}</span>
  );
}

function Skipped({ skipped }: Readonly<{ skipped: SkippedName[] }>) {
  if (skipped.length === 0) return null;
  return (
    <ul className="mt-1 list-disc pl-5 text-[0.84rem] text-ink-muted">
      {skipped.map((entry) => (
        <li key={entry.name}>
          <span className="font-mono">{entry.name}</span>: {skipReasonText(entry.reason)}
        </li>
      ))}
    </ul>
  );
}

/** ACME for one subsystem: the switch, what an order covers, and how it went. */
export default function AcmeCard({ subsystem, onToggle }: Readonly<Props>) {
  const canWrite = useCanWrite();
  const { push, notifyError } = useToast();
  const [busy, setBusy] = useState(false);
  const now = useNow(30_000);
  const view = useQuery({
    queryKey: ["acme", subsystem.id],
    queryFn: () => api.subsystemAcme(subsystem.id),
  });
  const overview = useQuery({ queryKey: ["acme"], queryFn: () => api.acme() });
  const data = view.data?.data;
  if (!data) return null;
  const enabled = subsystem.https.acmeEnabled;
  const status = data.status;
  const staging = overview.data?.data.staging === true;

  async function renew() {
    setBusy(true);
    try {
      await api.renewAcme(subsystem.id);
      push("success", "Renewal requested; the status updates when the order finishes");
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  }

  async function toggle(next: boolean) {
    if (!onToggle) return;
    setBusy(true);
    try {
      await onToggle(next);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="rounded-md border border-line p-3" aria-label="ACME">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-[0.95rem] font-semibold">Automatic certificate (ACME)</h3>
        {enabled && status ? (
          <Badge tone={acmeStateBadge(status.state).tone} dot>
            {acmeStateBadge(status.state).label}
          </Badge>
        ) : null}
        {staging ? <Badge tone="warn">staging CA</Badge> : null}
      </div>

      {data.available ? null : (
        <p role="note" className="mt-2 text-[0.84rem] text-ink-muted">
          ACME is off for this proxy. Set <span className="font-mono">enabled = true</span> and{" "}
          <span className="font-mono">allowed_domains</span> under{" "}
          <span className="font-mono">[data_plane.acme]</span> and restart to get certificates automatically.
        </p>
      )}

      {canWrite && onToggle && data.available ? (
        <div className="mt-2">
          <Toggle
            checked={enabled}
            disabled={busy}
            onChange={(next) => void toggle(next)}
            label="Get and renew this subsystem's certificate automatically"
          />
        </div>
      ) : null}

      {data.available && staging && enabled ? (
        <p className="mt-2 text-[0.84rem] text-warn">
          The proxy uses a staging CA: its certificates are not trusted by browsers.
        </p>
      ) : null}

      {data.preview && !status ? (
        <div className="mt-2 text-[0.86rem]">
          {enabled ? "The first order will cover: " : "An order would cover: "}
          <Names names={data.preview.eligibleNames} />
          <Skipped skipped={data.preview.skippedNames} />
        </div>
      ) : null}

      {enabled && status ? (
        <dl className="mt-2 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-[0.86rem]">
          <dt className="text-ink-muted">Names</dt>
          <dd>
            <Names names={status.eligibleNames} />
            <Skipped skipped={status.skippedNames} />
          </dd>
          {status.notAfterMs === null ? null : (
            <>
              <dt className="text-ink-muted">Certificate</dt>
              <dd>
                {status.certificateName ?? "issued"}, expires {relativeTime(status.notAfterMs, now)}
              </dd>
            </>
          )}
          {status.nextAttemptAtMs === null ? null : (
            <>
              <dt className="text-ink-muted">Next attempt</dt>
              <dd>{relativeTime(status.nextAttemptAtMs, now)}</dd>
            </>
          )}
          {status.lastError === "" ? null : (
            <>
              <dt className="text-ink-muted">Last error</dt>
              <dd className="break-words text-err">{status.lastError}</dd>
            </>
          )}
          {status.lastSuccessAtMs === null ? null : (
            <>
              <dt className="text-ink-muted">Last issued</dt>
              <dd>{relativeTime(status.lastSuccessAtMs, now)}</dd>
            </>
          )}
        </dl>
      ) : null}

      {enabled && data.available ? (
        <WriteOnly>
          <div className="mt-3">
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              disabled={busy}
              onClick={() => void renew()}
            >
              Renew now
            </button>
          </div>
        </WriteOnly>
      ) : null}
    </div>
  );
}
