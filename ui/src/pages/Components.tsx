// Example page: a gallery of every component style the theme ships,
// plus live demos of the error-handling pipeline (backend errors →
// toast bubbles, render crashes → error boundary).
import { useState, type ReactElement, type ReactNode } from "react";
import Badge from "../components/Badge";
import ErrorBoundary from "../components/ErrorBoundary";
import Toggle from "../components/Toggle";
import { useToast } from "../context/ToastContext";
import {
  IconAlertTriangle,
  IconBolt,
  IconCheckCircle,
  IconInfo,
  IconPlus,
  IconServer,
  IconXCircle,
} from "../icons";
import { api } from "../lib/api";

function Section({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <section className="card">
      <div className="card-head">
        <h2>{title}</h2>
        {hint ? <span className="card-hint">{hint}</span> : null}
      </div>
      {children}
    </section>
  );
}

function Alert({
  tone,
  icon,
  children,
}: {
  tone: "info" | "ok" | "warn" | "err";
  icon: ReactElement;
  children: ReactNode;
}) {
  const tones = {
    info: "bg-info-soft [&>span]:text-info",
    ok: "bg-ok-soft [&>span]:text-ok",
    warn: "bg-warn-soft [&>span]:text-warn",
    err: "bg-err-soft [&>span]:text-err",
  } as const;
  return (
    <div className={`flex items-start gap-2.5 rounded-lg p-3 text-sm ${tones[tone]}`}>
      <span className="mt-px inline-flex shrink-0">{icon}</span>
      <div>{children}</div>
    </div>
  );
}

/** Throws during render — recovered by the surrounding ErrorBoundary. */
function Bomb(): ReactElement {
  throw new Error("Demo render crash — recovered by the section's ErrorBoundary");
}

const DEPLOYS = [
  { name: "api-gateway", env: "prod", status: "Healthy", tone: "ok", latency: "38 ms", uptime: "99.99%" },
  { name: "billing-worker", env: "prod", status: "Degraded", tone: "warn", latency: "212 ms", uptime: "99.72%" },
  { name: "search-indexer", env: "staging", status: "Down", tone: "err", latency: "—", uptime: "97.10%" },
  { name: "web-frontend", env: "dev", status: "Healthy", tone: "ok", latency: "51 ms", uptime: "99.95%" },
] as const;

const TH = "border-b border-line bg-surface-2 px-4 py-2.5 text-left font-mono text-[0.68rem] font-semibold tracking-wider text-ink-faint uppercase";
const TD = "px-4 py-2.5";

export default function ComponentsPage() {
  const { push, notifyError } = useToast();
  const [notifications, setNotifications] = useState(true);
  const [exploded, setExploded] = useState(false);

  return (
    <div className="flex flex-col gap-4">
      <Section title="Buttons" hint="variants and sizes">
        <div className="flex flex-wrap items-center gap-2.5">
          <button className="btn btn-primary">Primary</button>
          <button className="btn btn-secondary">Secondary</button>
          <button className="btn btn-ghost">Ghost</button>
          <button className="btn btn-danger">Danger</button>
          <button className="btn btn-primary" disabled>
            Disabled
          </button>
          <button className="btn btn-primary">
            <IconPlus size={16} /> With icon
          </button>
          <button className="btn btn-secondary btn-sm">Small</button>
        </div>
      </Section>

      <Section title="Badges" hint="statuses and labels">
        <div className="flex flex-wrap items-center gap-2.5">
          <Badge>Neutral</Badge>
          <Badge tone="accent">Accent</Badge>
          <Badge tone="ok" dot>
            Operational
          </Badge>
          <Badge tone="warn" dot>
            Degraded
          </Badge>
          <Badge tone="err" dot>
            Down
          </Badge>
          <Badge tone="info">v0.1.0</Badge>
        </div>
      </Section>

      <Section title="Form controls">
        <div className="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))] gap-4">
          <label className="flex flex-col gap-1.5">
            <span className="text-[0.8rem] font-medium text-ink-muted">Name</span>
            <input className="input" placeholder="Ferris the crab" />
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-[0.8rem] font-medium text-ink-muted">Region</span>
            <select className="input" defaultValue="eu-central">
              <option value="us-east">us-east</option>
              <option value="eu-central">eu-central</option>
              <option value="ap-south">ap-south</option>
            </select>
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-[0.8rem] font-medium text-ink-muted">Notes</span>
            <textarea className="input" rows={2} placeholder="Optional" />
          </label>
          <div className="flex flex-col gap-1.5">
            <span className="text-[0.8rem] font-medium text-ink-muted">Toggles</span>
            <Toggle
              checked={notifications}
              onChange={setNotifications}
              label={notifications ? "Notifications on" : "Notifications off"}
            />
            <Toggle checked={false} onChange={() => undefined} label="Disabled" disabled />
          </div>
        </div>
      </Section>

      <Section title="Alerts" hint="inline, non-blocking">
        <div className="flex flex-col gap-2.5">
          <Alert tone="info" icon={<IconInfo size={18} />}>
            <strong className="font-semibold">Info.</strong> Config changes apply on server
            restart.
          </Alert>
          <Alert tone="ok" icon={<IconCheckCircle size={18} />}>
            <strong className="font-semibold">Success.</strong> The build finished in 4.2s.
          </Alert>
          <Alert tone="warn" icon={<IconAlertTriangle size={18} />}>
            <strong className="font-semibold">Warning.</strong> The access log is growing
            past 100 MB.
          </Alert>
          <Alert tone="err" icon={<IconXCircle size={18} />}>
            <strong className="font-semibold">Error.</strong> The database connection was
            refused.
          </Alert>
        </div>
      </Section>

      <Section title="Table" hint="scrolls horizontally on small screens">
        <div className="overflow-x-auto rounded-lg border border-line">
          <table className="w-full min-w-[540px] border-collapse text-sm">
            <thead>
              <tr>
                <th className={TH}>Service</th>
                <th className={TH}>Environment</th>
                <th className={TH}>Status</th>
                <th className={`${TH} text-right`}>Latency</th>
                <th className={`${TH} text-right`}>Uptime</th>
              </tr>
            </thead>
            <tbody>
              {DEPLOYS.map((row) => (
                <tr
                  key={row.name}
                  className="border-b border-line last:border-b-0 hover:bg-surface-2"
                >
                  <td className={TD}>
                    <code>{row.name}</code>
                  </td>
                  <td className={TD}>
                    <Badge tone={row.env === "prod" ? "accent" : "neutral"}>{row.env}</Badge>
                  </td>
                  <td className={TD}>
                    <Badge tone={row.tone} dot>
                      {row.status}
                    </Badge>
                  </td>
                  <td className={`${TD} text-right tabular-nums`}>{row.latency}</td>
                  <td className={`${TD} text-right tabular-nums`}>{row.uptime}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Section>

      <Section title="Progress & loading">
        <div className="flex flex-col gap-4">
          <div
            className="h-2 overflow-hidden rounded-full bg-surface-2"
            role="progressbar"
            aria-valuenow={64}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <span className="block h-full rounded-full bg-accent" style={{ width: "64%" }} />
          </div>
          <div className="flex flex-wrap items-center gap-2.5">
            <div className="spinner" />
            <div className="skeleton" style={{ width: 180 }} />
            <div className="skeleton" style={{ width: 120 }} />
          </div>
        </div>
      </Section>

      <Section title="Errors & notifications" hint="the full pipeline, live">
        <div className="flex flex-wrap items-center gap-2.5">
          <button
            className="btn btn-secondary"
            onClick={() => push("success", "Everything saved cleanly.")}
          >
            <IconCheckCircle size={16} /> Success toast
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => push("info", "Deploy started for web-frontend.")}
          >
            <IconInfo size={16} /> Info toast
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => api.errorDemo("internal").catch(notifyError)}
          >
            <IconServer size={16} /> Backend 500
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => api.errorDemo("bad-request").catch(notifyError)}
          >
            <IconAlertTriangle size={16} /> Backend 400
          </button>
          <button className="btn btn-secondary" onClick={() => api.missing().catch(notifyError)}>
            <IconXCircle size={16} /> Unknown endpoint
          </button>
        </div>
        <ErrorBoundary
          title="This section crashed (on purpose)"
          onRetry={() => setExploded(false)}
        >
          <div className="mt-3 flex flex-wrap items-center gap-2.5">
            {exploded ? (
              <Bomb />
            ) : (
              <button className="btn btn-danger" onClick={() => setExploded(true)}>
                <IconBolt size={16} /> Throw a render error
              </button>
            )}
          </div>
        </ErrorBoundary>
      </Section>
    </div>
  );
}
