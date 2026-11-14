import { Link } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import Badge from "../components/Badge";
import FormError from "../components/FormError";
import FormField, { errorProps } from "../components/FormField";
import { api } from "../lib/api";
import type { Resolution } from "../lib/types";
import { useServerErrors } from "../lib/useServerErrors";

const WHY_NOT: Record<NonNullable<Resolution["reason"]>, string> = {
  unknown_host: "No subsystem answers to this host, so the proxy replies 404.",
  subsystem_disabled: "The subsystem is disabled, so the proxy replies 503.",
  no_route: "The subsystem has no route matching this path, so the proxy replies 404.",
};

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="text-ink-muted">{label}</dt>
      <dd className="m-0 [overflow-wrap:anywhere]">{children}</dd>
    </>
  );
}

function ResolutionView({ resolution, version }: { resolution: Resolution; version: number }) {
  const { subsystem, route } = resolution;
  return (
    <section className="card">
      <div className="card-head">
        <h2>Result</h2>
        <Badge tone={resolution.matched ? "ok" : "warn"} dot>
          {resolution.matched ? "matched" : "no match"}
        </Badge>
        <span className="card-hint">against config version {version}</span>
      </div>
      {!resolution.matched && resolution.reason ? (
        <p className="mb-3 text-[0.9rem]">{WHY_NOT[resolution.reason]}</p>
      ) : null}
      <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-[0.88rem]">
        {subsystem ? (
          <Row label="Subsystem">
            <Link
              to="/subsystems/$id"
              params={{ id: String(subsystem.id) }}
              className="font-medium text-accent hover:underline"
            >
              {subsystem.name}
            </Link>
          </Row>
        ) : null}
        {resolution.https ? (
          <>
            <Row label="HTTPS">
              {resolution.https.servedOverHttps
                ? "served over HTTPS"
                : "not served over HTTPS (HTTPS is off for this subsystem, or TLS is off for the proxy)"}
            </Row>
            <Row label="Plain HTTP">
              {resolution.https.plainHttpRedirect ? (
                <>
                  redirected with {resolution.https.plainHttpRedirect.status} to{" "}
                  <code className="font-semibold">{resolution.https.plainHttpRedirect.location}</code>
                </>
              ) : (
                "served as usual (no redirect)"
              )}
            </Row>
          </>
        ) : null}
        {route ? (
          <>
            <Row label="Route">
              <code>{route.prefix}</code>
            </Row>
            <Row label="Upstream">
              {route.upstreamName} (<code>{route.algorithm}</code>), reached over{" "}
              <code>{route.upstreamScheme}</code>
            </Row>
            <Row label="Rewrite">
              {[
                route.stripPrefix ? "strip prefix" : null,
                route.upstreamPrefix ? `prepend ${route.upstreamPrefix}` : null,
                route.preserveHost ? null : "rewrite Host header",
              ]
                .filter(Boolean)
                .join(", ") || "none"}
            </Row>
          </>
        ) : null}
        {route
          ? route.endpoints.map((endpoint) => (
              <Row key={endpoint.id} label={endpoint.eligible ? "Forwarded to" : "Not eligible"}>
                {endpoint.upstreamUrl ? (
                  <code className="font-semibold">{endpoint.upstreamUrl}</code>
                ) : (
                  <code>{endpoint.addr}</code>
                )}
                {endpoint.state === "enabled" ? null : ` (${endpoint.state})`}
              </Row>
            ))
          : null}
      </dl>
    </section>
  );
}

export default function ResolverPage() {
  const [host, setHost] = useState("");
  const [path, setPath] = useState("/");
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ resolution: Resolution; version: number } | null>(null);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function submit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const errors: Record<string, string> = {};
    if (host.trim() === "") errors.host = "Enter the host name a client would send";
    if (!path.startsWith("/")) errors.path = "Must start with '/'";
    setClientErrors(errors);
    if (Object.keys(errors).length > 0) return;
    setBusy(true);
    try {
      const response = await api.resolve(host.trim(), path);
      setResult({ resolution: response.data, version: response.version });
    } catch (thrown) {
      setResult(null);
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex max-w-[960px] flex-col gap-4">
      <section className="card">
        <div className="card-head">
          <h2>Try a request</h2>
          <span className="card-hint">dry run against the live configuration</span>
        </div>
        <form onSubmit={submit} noValidate className="flex flex-col gap-4">
          <FormError message={server.general} />
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <FormField label="Host" htmlFor="resolve-host" error={error("host")}>
              <input
                id="resolve-host"
                className="input"
                value={host}
                placeholder="shop.example.com"
                onChange={(event) => setHost(event.target.value)}
                autoComplete="off"
                {...errorProps("resolve-host", error("host"))}
              />
            </FormField>
            <FormField
              label="Path"
              htmlFor="resolve-path"
              error={error("path")}
              hint="May include a query string."
            >
              <input
                id="resolve-path"
                className="input font-mono"
                value={path}
                onChange={(event) => setPath(event.target.value)}
                autoComplete="off"
                {...errorProps("resolve-path", error("path"))}
              />
            </FormField>
          </div>
          <div>
            <button type="submit" className="btn btn-primary" disabled={busy}>
              Resolve
            </button>
          </div>
        </form>
      </section>
      {result ? <ResolutionView {...result} /> : null}
    </div>
  );
}
