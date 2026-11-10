import { useState, type FormEvent } from "react";
import {
  defaultPolicyValues,
  policyValuesFrom,
  validateEndpoint,
  validatePolicy,
  type FieldErrors,
  type PolicyValues,
} from "../../lib/policyValidation";
import type { EndpointInput, Upstream, UpstreamSettings } from "../../lib/types";
import { useServerErrors } from "../../lib/useServerErrors";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";
import PolicyFields from "./PolicyFields";
import UpstreamTlsFields from "./UpstreamTlsFields";

interface UpstreamFormProps {
  /** The group being edited; omit to create one. */
  initial?: Upstream;
  submitLabel: string;
  /** `firstEndpoint` is only given when creating: a group needs one endpoint. */
  onSubmit: (settings: UpstreamSettings, firstEndpoint: EndpointInput | null) => Promise<void>;
  onCancel: () => void;
}

/** The fields inside the folded "advanced" section. */
const ADVANCED_FIELDS = [
  "health.check",
  "health.path",
  "health.expectedStatus",
  "health.intervalMs",
  "health.timeoutMs",
  "health.rise",
  "health.fall",
  "passive.consecutiveFailures",
  "passive.baseEjectionMs",
  "passive.maxEjectionMs",
  "passive.maxEjectionPercent",
  "retryBudget.percent",
  "retryBudget.minPerSec",
];

/** An upstream group: its name and policy, and when creating, its first endpoint. */
export default function UpstreamForm({ initial, submitLabel, onSubmit, onCancel }: UpstreamFormProps) {
  const creating = initial === undefined;
  const [name, setName] = useState(initial?.name ?? "");
  const [addr, setAddr] = useState("");
  const [weight, setWeight] = useState("1");
  const [policy, setPolicy] = useState<PolicyValues>(
    initial ? policyValuesFrom(initial) : defaultPolicyValues(),
  );
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);
  const advancedError = ADVANCED_FIELDS.some((field) => error(field) !== undefined);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const errors: FieldErrors = {};
    const trimmed = name.trim();
    if (trimmed === "" || trimmed.length > 63) errors.name = "Must be 1 to 63 characters";
    const checked = validatePolicy(policy);
    Object.assign(errors, checked.errors);
    const endpoint = creating ? validateEndpoint({ addr, weight }, "endpoints.0.") : undefined;
    Object.assign(errors, endpoint?.errors);
    setClientErrors(errors);
    if (Object.keys(errors).length > 0 || !checked.value) return;
    setBusy(true);
    try {
      await onSubmit({ name: trimmed, ...checked.value }, endpoint?.value ?? null);
    } catch (thrown) {
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-4">
      <FormError message={server.general} />
      <FormField label="Name" htmlFor="upstream-name" error={error("name")}>
        <input
          id="upstream-name"
          className="input"
          value={name}
          onChange={(event) => setName(event.target.value)}
          autoComplete="off"
          {...errorProps("upstream-name", error("name"))}
        />
      </FormField>
      {creating ? (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-[1fr_8rem]">
          <FormField
            label="First endpoint address"
            htmlFor="upstream-addr"
            error={error("endpoints.0.addr")}
            hint="host:port, for example 127.0.0.1:3000 or backend.internal:8080. Add more endpoints afterwards."
          >
            <input
              id="upstream-addr"
              className="input"
              value={addr}
              onChange={(event) => setAddr(event.target.value)}
              autoComplete="off"
              {...errorProps("upstream-addr", error("endpoints.0.addr"))}
            />
          </FormField>
          <FormField label="Weight" htmlFor="upstream-weight" error={error("endpoints.0.weight")}>
            <input
              id="upstream-weight"
              className="input"
              inputMode="numeric"
              value={weight}
              onChange={(event) => setWeight(event.target.value)}
              {...errorProps("upstream-weight", error("endpoints.0.weight"))}
            />
          </FormField>
        </div>
      ) : null}
      <UpstreamTlsFields
        values={policy}
        onChange={(patch) => setPolicy((current) => ({ ...current, ...patch }))}
        error={error}
        idPrefix="upstream"
      />
      <PolicyFields
        values={policy}
        onChange={(patch) => setPolicy((current) => ({ ...current, ...patch }))}
        error={error}
        idPrefix="upstream"
        openAdvanced={advancedError}
      />
      <div className="flex justify-end gap-2">
        <button type="button" className="btn btn-secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}
