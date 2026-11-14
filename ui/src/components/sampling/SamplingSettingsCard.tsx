// The global sampling settings, on the Settings page. A viewer reads them; an
// admin edits them. The server re-validates every field and shows its errors
// next to the inputs.
import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useCanWrite } from "../../context/SessionContext";
import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import { formatBytes, formatNumber, timeAgo } from "../../lib/format";
import {
  baselineWords,
  TRIGGERS,
  validateSampling,
  valuesOf,
  type SamplingValues,
} from "../../lib/samplingValidation";
import type { SampleTrigger, SamplingView } from "../../lib/types";
import { SAMPLING_KEY, useSampling } from "../../lib/useSampling";
import { useServerErrors } from "../../lib/useServerErrors";
import Badge from "../Badge";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";
import Toggle from "../Toggle";

type TextField = Exclude<keyof SamplingValues, "enabled" | "triggers">;

function Status({ view }: { view: SamplingView }) {
  const { status } = view;
  return (
    <dl className="mb-4 grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
      <div>
        <dt className="text-ink-faint">This process</dt>
        <dd className="m-0 font-medium">
          {status.processEnabled ? "samples" : "does not sample (config.toml)"}
        </dd>
      </div>
      <div>
        <dt className="text-ink-faint">Captured</dt>
        <dd className="m-0 font-medium">{formatNumber(status.captured)}</dd>
      </div>
      <div>
        <dt className="text-ink-faint">Dropped</dt>
        <dd className="m-0 font-medium">{formatNumber(status.dropped)}</dd>
      </div>
      <div>
        <dt className="text-ink-faint">Stored</dt>
        <dd className="m-0 font-medium">
          {status.table
            ? `${formatNumber(status.table.rows)} rows, ${formatBytes(status.table.bytes)}`
            : "unknown"}
        </dd>
      </div>
    </dl>
  );
}

function Redaction({ view }: { view: SamplingView }) {
  const built = view.settings.builtInRedaction;
  return (
    <p className="text-[0.76rem] leading-relaxed text-ink-faint">
      Always redacted, and not editable: headers <code>{built.headers.join(", ")}</code>; query
      parameters <code>{built.queryParams.join(", ")}</code>; body fields{" "}
      <code>{built.bodyFields.join(", ")}</code>. The lists above only add to these.
    </p>
  );
}

function Form({ view }: { view: SamplingView }) {
  const canWrite = useCanWrite();
  const queryClient = useQueryClient();
  const { push, notifyError } = useToast();
  const [values, setValues] = useState<SamplingValues>(() => valuesOf(view.settings));
  const [clientErrors, setClientErrors] = useState<Record<string, string | undefined>>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);
  const readOnly = !canWrite;

  const text = (field: TextField, apiField: string, label: string, hint?: string, unit?: string) => (
    <FormField label={unit ? `${label} (${unit})` : label} htmlFor={`sampling-${field}`} error={error(apiField)} hint={hint}>
      <input
        id={`sampling-${field}`}
        className="input"
        inputMode="decimal"
        value={values[field]}
        readOnly={readOnly}
        onChange={(event) => setValues({ ...values, [field]: event.target.value })}
        {...errorProps(`sampling-${field}`, error(apiField))}
      />
    </FormField>
  );

  const list = (field: TextField, label: string, hint: string) => (
    <FormField label={label} htmlFor={`sampling-${field}`} error={error(field)} hint={hint}>
      <textarea
        id={`sampling-${field}`}
        className="input min-h-[4.5rem] font-mono"
        value={values[field]}
        readOnly={readOnly}
        onChange={(event) => setValues({ ...values, [field]: event.target.value })}
        {...errorProps(`sampling-${field}`, error(field))}
      />
    </FormField>
  );

  function toggleTrigger(trigger: SampleTrigger, on: boolean) {
    const rest = values.triggers.filter((entry) => entry !== trigger);
    setValues({ ...values, triggers: on ? [...rest, trigger] : rest });
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateSampling(values);
    setClientErrors(result.errors);
    if (result.value === undefined) return;
    setBusy(true);
    try {
      await api.putSampling(result.value);
      await queryClient.invalidateQueries({ queryKey: [SAMPLING_KEY] });
      push("success", "Sampling settings saved");
    } catch (thrown) {
      try {
        server.capture(thrown);
      } catch {
        notifyError(thrown);
      }
    } finally {
      setBusy(false);
    }
  }

  const baseline = Number(values.baselinePerMille);
  return (
    <form onSubmit={submit} noValidate className="flex flex-col gap-4">
      <FormError message={server.general} />
      <Toggle
        checked={values.enabled}
        disabled={readOnly}
        label="Sample unusual requests"
        onChange={(enabled) => setValues({ ...values, enabled })}
      />

      <fieldset className="flex flex-col gap-2" disabled={readOnly}>
        <legend className="mb-1 text-[0.82rem] font-medium">What gets sampled</legend>
        {TRIGGERS.map((trigger) => (
          <label key={trigger.value} className="flex items-start gap-2 text-[0.86rem]">
            <input
              type="checkbox"
              className="mt-1"
              checked={values.triggers.includes(trigger.value)}
              onChange={(event) => toggleTrigger(trigger.value, event.target.checked)}
            />
            <span>
              {trigger.label}
              <span className="block text-[0.76rem] text-ink-faint">{trigger.hint}</span>
            </span>
          </label>
        ))}
        {error("triggers") ? <p role="alert" className="text-[0.78rem] text-err">{error("triggers")}</p> : null}
      </fieldset>

      <div className="grid gap-3 sm:grid-cols-2">
        {text("slowThresholdMs", "slowThresholdMs", "Slow after", undefined, "ms")}
        {text("slowP99Factor", "slowP99FactorX10", "Slower than usual by", "A multiple of the route's p99, such as 3.", "times")}
        {text("slowP99FloorMs", "slowP99FloorMs", "…but at least", "Ignores routes that are always quick.", "ms")}
        {text("clientErrorsPerMin", "clientErrorsPerMin", "Failing client after", "4xx answers to one address in a minute.", "per minute")}
        {text("oversizedRequestMiB", "oversizedRequestBytes", "Large request from", undefined, "MiB")}
        {text("oversizedResponseMiB", "oversizedResponseBytes", "Large response from", undefined, "MiB")}
        {text(
          "baselinePerMille",
          "baselinePerMille",
          "Baseline",
          Number.isFinite(baseline) ? `Keeps ${baselineWords(baseline)}.` : undefined,
          "per thousand",
        )}
      </div>

      <h3 className="mt-2 text-[0.9rem] font-semibold">Limits during an incident</h3>
      <div className="grid gap-3 sm:grid-cols-2">
        {text("ratePerSec", "ratePerSec", "At most", "Samples stored per second by one instance.", "per second")}
        {text("rateBurst", "rateBurst", "Burst", undefined, "samples")}
        {text("stormBurst", "stormBurst", "Same problem: first", "Samples kept of one kind (route, trigger, endpoint) before slowing down.", "samples")}
        {text("stormIntervalSecs", "stormIntervalSecs", "…then one every", undefined, "seconds")}
      </div>

      <h3 className="mt-2 text-[0.9rem] font-semibold">Retention</h3>
      <div className="grid gap-3 sm:grid-cols-3">
        {text("retentionDays", "retentionDays", "Keep for", undefined, "days")}
        {text("maxRows", "maxRows", "At most", undefined, "samples")}
        {text("maxMiB", "maxBytes", "At most", undefined, "MiB")}
      </div>

      <h3 className="mt-2 text-[0.9rem] font-semibold">Bodies and redaction</h3>
      <p className="text-[0.78rem] text-ink-faint">
        Bodies are off everywhere until a route turns them on (on its page). Only text types are
        kept, cut at these sizes, after redaction.
      </p>
      <div className="grid gap-3 sm:grid-cols-2">
        {text("bodyRequestKiB", "bodyRequestCap", "Request body up to", undefined, "KiB")}
        {text("bodyResponseKiB", "bodyResponseCap", "Response body up to", undefined, "KiB")}
      </div>
      {list("extraHeaders", "Also keep these request headers", "One per line. Others are kept only as names.")}
      {list("redactHeaders", "Also redact these headers", "One per line.")}
      {list("redactQueryParams", "Also redact these query parameters", "One per line.")}
      {list("redactBodyFields", "Also redact these body fields", "One per line, matched in JSON, forms and XML.")}
      <Redaction view={view} />

      {canWrite ? (
        <div className="flex items-center justify-end gap-3">
          <span className="text-[0.76rem] text-ink-faint">
            last saved by {view.settings.updatedBy || "nobody yet"}
            {view.settings.updatedAtMs ? `, ${timeAgo(view.settings.updatedAtMs)}` : ""}
          </span>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            Save sampling settings
          </button>
        </div>
      ) : null}
    </form>
  );
}

export default function SamplingSettingsCard() {
  const sampling = useSampling();
  const view = sampling.data?.data;
  return (
    <section className="card">
      <div className="card-head">
        <h2>Request sampling</h2>
        <span className="card-hint">keeps the unusual requests for a look</span>
        {view ? <Badge tone={view.settings.enabled ? "ok" : "neutral"} dot>{view.settings.enabled ? "on" : "off"}</Badge> : null}
      </div>
      {sampling.isError ? (
        <p className="text-[0.9rem] text-err">Could not load the sampling settings: {sampling.error.message}</p>
      ) : !view ? (
        <div className="flex items-center gap-3 text-ink-faint">
          <div className="spinner" /> Loading…
        </div>
      ) : (
        <>
          <Status view={view} />
          {/* keyed by the save time so a change made elsewhere resets the form */}
          <Form key={`${view.settings.updatedAtMs}-${view.settings.updatedBy}`} view={view} />
        </>
      )}
    </section>
  );
}
