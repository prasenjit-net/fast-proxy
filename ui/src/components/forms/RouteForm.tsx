import { useState, type FormEvent } from "react";
import type { RouteInput, Upstream } from "../../lib/types";
import { useServerErrors } from "../../lib/useServerErrors";
import { validateRoute, type FieldErrors } from "../../lib/validation";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";
import Toggle from "../Toggle";

interface RouteFormProps {
  /** The subsystem's upstreams, offered as the route's destination. */
  upstreams: Upstream[];
  initial?: RouteInput;
  submitLabel: string;
  onSubmit: (input: RouteInput) => Promise<void>;
  onCancel: () => void;
}

/** Everything about a route: where it matches, where it goes, and how. */
export default function RouteForm({ upstreams, initial, submitLabel, onSubmit, onCancel }: RouteFormProps) {
  const [prefix, setPrefix] = useState(initial?.prefix ?? "/");
  const [upstreamGroupId, setUpstreamGroupId] = useState(
    String(initial?.upstreamGroupId ?? upstreams[0]?.id ?? ""),
  );
  const [stripPrefix, setStripPrefix] = useState(initial?.stripPrefix ?? false);
  const [upstreamPrefix, setUpstreamPrefix] = useState(initial?.upstreamPrefix ?? "");
  const [preserveHost, setPreserveHost] = useState(initial?.preserveHost ?? true);
  const [connectTimeoutMs, setConnectTimeoutMs] = useState(String(initial?.connectTimeoutMs ?? 5000));
  const [responseTimeoutMs, setResponseTimeoutMs] = useState(String(initial?.responseTimeoutMs ?? 30000));
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateRoute({
      prefix,
      upstreamGroupId,
      stripPrefix,
      upstreamPrefix,
      preserveHost,
      connectTimeoutMs,
      responseTimeoutMs,
      enabled,
    });
    setClientErrors(result.errors);
    if (!result.value) return;
    setBusy(true);
    try {
      await onSubmit(result.value);
    } catch (thrown) {
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }

  const noUpstreams = upstreams.length === 0;

  return (
    <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-4">
      <FormError message={server.general} />
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <FormField
          label="Path prefix"
          htmlFor="route-prefix"
          error={error("prefix")}
          hint="Matches this path and everything below it. The longest prefix wins."
        >
          <input
            id="route-prefix"
            className="input font-mono"
            value={prefix}
            onChange={(event) => setPrefix(event.target.value)}
            autoComplete="off"
            {...errorProps("route-prefix", error("prefix"))}
          />
        </FormField>
        <FormField
          label="Upstream"
          htmlFor="route-upstream"
          error={error("upstreamGroupId")}
          hint={noUpstreams ? "Add an upstream to this subsystem first." : undefined}
        >
          <select
            id="route-upstream"
            className="input"
            value={upstreamGroupId}
            onChange={(event) => setUpstreamGroupId(event.target.value)}
            disabled={noUpstreams}
            {...errorProps("route-upstream", error("upstreamGroupId"))}
          >
            {upstreams.map((upstream) => (
              <option key={upstream.id} value={upstream.id}>
                {upstream.name} ({upstream.addr})
              </option>
            ))}
          </select>
        </FormField>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <FormField
          label="Upstream path prefix"
          htmlFor="route-upstream-prefix"
          error={error("upstreamPrefix")}
          hint="Put in front of the forwarded path. Optional."
        >
          <input
            id="route-upstream-prefix"
            className="input font-mono"
            value={upstreamPrefix}
            placeholder="/v2"
            onChange={(event) => setUpstreamPrefix(event.target.value)}
            autoComplete="off"
            {...errorProps("route-upstream-prefix", error("upstreamPrefix"))}
          />
        </FormField>
        <div className="flex flex-col justify-end gap-3 pb-1">
          <Toggle checked={stripPrefix} onChange={setStripPrefix} label="Strip the matched prefix" />
          <Toggle checked={preserveHost} onChange={setPreserveHost} label="Keep the client's Host header" />
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <FormField
          label="Connect timeout (ms)"
          htmlFor="route-connect-timeout"
          error={error("connectTimeoutMs")}
        >
          <input
            id="route-connect-timeout"
            className="input"
            inputMode="numeric"
            value={connectTimeoutMs}
            onChange={(event) => setConnectTimeoutMs(event.target.value)}
            {...errorProps("route-connect-timeout", error("connectTimeoutMs"))}
          />
        </FormField>
        <FormField
          label="Response timeout (ms)"
          htmlFor="route-response-timeout"
          error={error("responseTimeoutMs")}
          hint="Waiting for the upstream to start answering."
        >
          <input
            id="route-response-timeout"
            className="input"
            inputMode="numeric"
            value={responseTimeoutMs}
            onChange={(event) => setResponseTimeoutMs(event.target.value)}
            {...errorProps("route-response-timeout", error("responseTimeoutMs"))}
          />
        </FormField>
      </div>

      <Toggle checked={enabled} onChange={setEnabled} label="Enabled" />
      <div className="flex justify-end gap-2">
        <button type="button" className="btn btn-secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary" disabled={busy || noUpstreams}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}
