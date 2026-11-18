// The rate limit of a subsystem or route: a badge that says what applies, and
// (for a writer) a dialog to set or remove it. Limits are per instance (ADR 0030).
import { useState, type FormEvent } from "react";
import { useCanWrite } from "../../context/SessionContext";
import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import {
  defaultRateLimitValues,
  rateLimitSummary,
  rateLimitValuesFrom,
  validateRateLimit,
  type PeriodUnit,
  type RateLimitValues,
} from "../../lib/rateLimitValidation";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import { useServerErrors } from "../../lib/useServerErrors";
import type { FieldErrors } from "../../lib/validation";
import type { RateLimit, RateLimitTarget } from "../../lib/types";
import Badge from "../Badge";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";
import Modal from "../Modal";

interface RateLimitControlProps {
  target: RateLimitTarget;
  /** The policy set on this target itself. */
  current: RateLimit | null;
  /** For a route: its subsystem's policy, which applies when the route has none. */
  inherited?: RateLimit | null;
  /** Names the target in the button and the dialog, such as `route /api`. */
  label: string;
}

function RateLimitDialog(props: RateLimitControlProps & { onClose: () => void }) {
  const refresh = useRefreshConfigData();
  const { push } = useToast();
  const [values, setValues] = useState<RateLimitValues>(() =>
    props.current ? rateLimitValuesFrom(props.current) : defaultRateLimitValues(),
  );
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);
  const set = (patch: Partial<RateLimitValues>) => setValues((now) => ({ ...now, ...patch }));

  async function run(action: () => Promise<unknown>, done: string) {
    setBusy(true);
    try {
      await action();
      await refresh();
      push("success", done);
      props.onClose();
    } catch (thrown) {
      server.capture(thrown);
      setBusy(false);
    }
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateRateLimit(values);
    setClientErrors(result.errors);
    const input = result.value;
    if (!input) return;
    await run(() => api.setRateLimit(props.target, input), `Rate limit saved for ${props.label}`);
  }

  const remove = () =>
    run(() => api.clearRateLimit(props.target), `Rate limit removed from ${props.label}`);

  return (
    <Modal title={`Rate limit for ${props.label}`} onClose={props.onClose}>
      <form onSubmit={save} noValidate className="flex flex-col gap-4">
        <FormError message={server.general} />
        <p className="text-[0.84rem] text-ink-muted">
          Counted on each instance separately: three instances let up to three times the rate
          through. Requests over the limit get a 429 with Retry-After and never reach an upstream.
        </p>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <FormField label="Requests (per instance)" htmlFor="rate-limit-rate" error={error("rateLimit.rate")}>
            <input
              id="rate-limit-rate"
              className="input"
              inputMode="numeric"
              value={values.rate}
              onChange={(event) => set({ rate: event.target.value })}
              {...errorProps("rate-limit-rate", error("rateLimit.rate"))}
            />
          </FormField>
          <FormField label="Every" htmlFor="rate-limit-period" error={error("rateLimit.periodSecs")}>
            <div className="flex items-center gap-2">
              <input
                id="rate-limit-period"
                className="input"
                inputMode="numeric"
                value={values.periodAmount}
                onChange={(event) => set({ periodAmount: event.target.value })}
                {...errorProps("rate-limit-period", error("rateLimit.periodSecs"))}
              />
              <select
                aria-label="Period unit"
                className="input"
                value={values.periodUnit}
                onChange={(event) => set({ periodUnit: event.target.value as PeriodUnit })}
              >
                <option value="s">seconds</option>
                <option value="m">minutes</option>
                <option value="h">hours</option>
              </select>
            </div>
          </FormField>
        </div>
        <FormField
          label="Burst"
          htmlFor="rate-limit-burst"
          error={error("rateLimit.burst")}
          hint="Requests allowed at once after a quiet spell. Empty is the same as the rate."
        >
          <input
            id="rate-limit-burst"
            className="input"
            inputMode="numeric"
            value={values.burst}
            onChange={(event) => set({ burst: event.target.value })}
            {...errorProps("rate-limit-burst", error("rateLimit.burst"))}
          />
        </FormField>
        <FormField label="Count per" htmlFor="rate-limit-key">
          <select
            id="rate-limit-key"
            className="input"
            value={values.key}
            onChange={(event) => set({ key: event.target.value as RateLimitValues["key"] })}
          >
            <option value="client">client address</option>
            <option value="header">value of a request header</option>
          </select>
        </FormField>
        {values.key === "header" ? (
          <FormField
            label="Header name"
            htmlFor="rate-limit-header"
            error={error("rateLimit.header")}
            hint="Requests without it share one budget. Its values are hashed and never stored."
          >
            <input
              id="rate-limit-header"
              className="input font-mono"
              value={values.header}
              placeholder="X-Api-Key"
              autoComplete="off"
              onChange={(event) => set({ header: event.target.value })}
              {...errorProps("rate-limit-header", error("rateLimit.header"))}
            />
          </FormField>
        ) : (
          <FormField
            label="IPv6 prefix length"
            htmlFor="rate-limit-ipv6"
            error={error("rateLimit.ipv6Prefix")}
            hint="IPv6 clients are counted per network of this size; 64 is one household or host."
          >
            <input
              id="rate-limit-ipv6"
              className="input"
              inputMode="numeric"
              value={values.ipv6Prefix}
              onChange={(event) => set({ ipv6Prefix: event.target.value })}
              {...errorProps("rate-limit-ipv6", error("rateLimit.ipv6Prefix"))}
            />
          </FormField>
        )}
        <div className="flex flex-wrap justify-end gap-2">
          {props.current ? (
            <button type="button" className="btn btn-danger mr-auto" onClick={() => void remove()} disabled={busy}>
              Remove limit
            </button>
          ) : null}
          <button type="button" className="btn btn-secondary" onClick={props.onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            Save limit
          </button>
        </div>
      </form>
    </Modal>
  );
}

/** What limits `target`, and for a writer a way to change it. */
export default function RateLimitControl(props: RateLimitControlProps) {
  const canWrite = useCanWrite();
  const [editing, setEditing] = useState(false);
  const { current, inherited } = props;
  let badge = <Badge tone="neutral">no rate limit</Badge>;
  if (current) {
    badge = <Badge tone="accent">{rateLimitSummary(current)}</Badge>;
  } else if (inherited) {
    badge = <Badge tone="neutral">subsystem's: {rateLimitSummary(inherited)}</Badge>;
  }
  return (
    <span className="inline-flex items-center gap-2">
      {badge}
      {canWrite ? (
        <button
          className="btn btn-secondary btn-sm"
          aria-label={`Edit rate limit for ${props.label}`}
          onClick={() => setEditing(true)}
        >
          Limit
        </button>
      ) : null}
      {editing ? <RateLimitDialog {...props} onClose={() => setEditing(false)} /> : null}
    </span>
  );
}
