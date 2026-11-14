import { useMutation, useQuery } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import SamplingSettingsCard from "../components/sampling/SamplingSettingsCard";
import { useConfig } from "../context/ConfigContext";
import { useLive } from "../context/LiveContext";
import { useSession } from "../context/SessionContext";
import { useTheme, type ThemeMode } from "../context/ThemeContext";
import { useToast } from "../context/ToastContext";
import { IconMonitor, IconMoon, IconServer, IconSun } from "../icons";
import { api } from "../lib/api";
import { formatUptime } from "../lib/format";

const MODES: { mode: ThemeMode; label: string; icon: ReactElement }[] = [
  { mode: "light", label: "Light", icon: <IconSun size={16} /> },
  { mode: "auto", label: "System", icon: <IconMonitor size={16} /> },
  { mode: "dark", label: "Dark", icon: <IconMoon size={16} /> },
];

function ConfigRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-line py-2 text-sm last:border-b-0">
      <dt className="shrink-0 text-ink-muted">{label}</dt>
      <dd className="m-0 text-right font-medium [overflow-wrap:anywhere]">{children}</dd>
    </div>
  );
}

export default function SettingsPage() {
  const config = useConfig();
  const { mode, setMode } = useTheme();
  const { status, traffic } = useLive();
  const { push, notifyError } = useToast();
  const { who } = useSession();
  const adminStatus = useQuery({ queryKey: ["status"], queryFn: api.status });
  const admin = adminStatus.data?.data.admin;

  const healthCheck = useMutation({
    mutationFn: api.health,
    onSuccess: (health) => push("success", `Server is ${health.status} — v${health.version}`),
    onError: notifyError,
  });

  return (
    <div className="flex flex-col gap-4">
      <div className="card-grid">
        <div className="flex flex-col gap-4">
          <section className="card">
            <div className="card-head">
              <h2>Appearance</h2>
              <span className="card-hint">stored in this browser</span>
            </div>
            <div className="inline-flex gap-1 self-start rounded-lg bg-surface-2 p-1">
              {MODES.map((entry) => (
                <button
                  key={entry.mode}
                  className={`inline-flex cursor-pointer items-center gap-1.5 rounded-md px-3.5 py-1.5 text-[0.85rem] font-medium transition-colors ${
                    mode === entry.mode
                      ? "bg-surface text-ink shadow-sm"
                      : "text-ink-muted hover:text-ink"
                  }`}
                  onClick={() => setMode(entry.mode)}
                >
                  {entry.icon} {entry.label}
                </button>
              ))}
            </div>
            <p className="mt-3 text-[0.8rem] leading-relaxed text-ink-faint">
              “System” follows the operating system preference. Until a choice is saved
              here, the UI uses the server-side default (<code>{config.ui.defaultTheme}</code>).
            </p>
          </section>
          <section className="card">
            <div className="card-head">
              <h2>Server configuration</h2>
              <span className="card-hint">from config.toml</span>
            </div>
            <dl className="mb-1 flex flex-col">
              <ConfigRow label="App name">{config.ui.appName}</ConfigRow>
              <ConfigRow label="Tagline">{config.ui.tagline}</ConfigRow>
              <ConfigRow label="Default theme">{config.ui.defaultTheme}</ConfigRow>
              <ConfigRow label="Server version">v{config.version}</ConfigRow>
              <ConfigRow label="Server started">
                {new Date(config.startedAtMs).toLocaleString()}
              </ConfigRow>
              <ConfigRow label="Repository">
                {config.ui.repoUrl ? (
                  <a
                    className="text-accent hover:underline"
                    href={config.ui.repoUrl}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {config.ui.repoUrl}
                  </a>
                ) : (
                  "—"
                )}
              </ConfigRow>
            </dl>
            <p className="mt-3 text-[0.8rem] leading-relaxed text-ink-faint">
              Served by <code>GET /api/config</code>. Edit <code>config.toml</code> or pass CLI
              flags (e.g. <code>--admin-listen 127.0.0.1:9100</code>) to change these — no UI rebuild needed.
            </p>
          </section>
        </div>
        <div className="flex flex-col gap-4">
          <section className="card">
            <div className="card-head">
              <h2>Admin access</h2>
              <span className="card-hint">how this admin plane is protected</span>
            </div>
            <dl className="mb-1 flex flex-col">
              <ConfigRow label="Signed in as">
                {who.name} ({who.role})
              </ConfigRow>
              <ConfigRow label="Authentication">
                {admin ? (admin.auth === "required" ? "required: a token for every call" : "off (loopback only)") : "…"}
              </ConfigRow>
              <ConfigRow label="Private keys">
                {admin
                  ? admin.protectedChannel
                    ? "accepted: the channel is protected (loopback or TLS)"
                    : "refused: this channel is not protected"
                  : "…"}
              </ConfigRow>
              <ConfigRow label="Admin TLS certificate">
                {admin ? (admin.tlsCertificate ?? "none: plain HTTP") : "…"}
              </ConfigRow>
              <ConfigRow label="Active tokens">{admin ? admin.activeTokens : "…"}</ConfigRow>
              <ConfigRow label="Token expires">
                {who.tokenExpiresAtMs === null ? "never" : new Date(who.tokenExpiresAtMs).toLocaleString()}
              </ConfigRow>
            </dl>
            {admin?.auth === "off" ? (
              <p role="note" className="mt-3 rounded-md bg-warn-soft p-3 text-[0.84rem] text-warn">
                Authentication is off, so anyone who can reach this page can change the configuration.
                That is allowed only on a loopback address.
              </p>
            ) : null}
          </section>
          <section className="card">
            <div className="card-head">
              <h2>Connection</h2>
              <span className="card-hint">live status</span>
            </div>
            <dl className="mb-4 flex flex-col">
              <ConfigRow label="WebSocket">{status}</ConfigRow>
              <ConfigRow label="Open proxy connections">
                {traffic ? traffic.activeConnections : "—"}
              </ConfigRow>
              <ConfigRow label="Server uptime">
                {formatUptime(Math.max(0, Math.floor((Date.now() - config.startedAtMs) / 1000)))}
              </ConfigRow>
            </dl>
            <button
              className="btn btn-secondary"
              onClick={() => healthCheck.mutate()}
              disabled={healthCheck.isPending}
            >
              <IconServer size={16} /> Check server health
            </button>
          </section>
        </div>
      </div>
      <SamplingSettingsCard />
    </div>
  );
}
