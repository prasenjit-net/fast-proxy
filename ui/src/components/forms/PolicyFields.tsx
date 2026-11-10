import { useState } from "react";
import { LB_ALGORITHMS, type HashKind, type PolicyValues } from "../../lib/policyValidation";
import FormField, { errorProps } from "../FormField";

interface PolicyFieldsProps {
  values: PolicyValues;
  onChange: (patch: Partial<PolicyValues>) => void;
  error: (field: string) => string | undefined;
  /** Prefix for element ids, so two forms never clash. */
  idPrefix: string;
  /** Open the advanced section, for example because it holds an error. */
  openAdvanced: boolean;
}

interface NumberFieldProps {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  hint?: string;
}

function NumberField({ id, label, value, onChange, error, hint }: NumberFieldProps) {
  return (
    <FormField label={label} htmlFor={id} error={error} hint={hint}>
      <input
        id={id}
        className="input"
        inputMode="numeric"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        {...errorProps(id, error)}
      />
    </FormField>
  );
}

/** Where consistent hashing reads its key from. */
function HashKeyFields({ values, onChange, error, idPrefix }: Omit<PolicyFieldsProps, "openAdvanced">) {
  const nameId = `${idPrefix}-hash-name`;
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <FormField label="Hash on" htmlFor={`${idPrefix}-hash-kind`}>
        <select
          id={`${idPrefix}-hash-kind`}
          className="input"
          value={values.hashKind}
          onChange={(event) => onChange({ hashKind: event.target.value as HashKind })}
        >
          <option value="client_ip">Client IP address</option>
          <option value="header">A request header</option>
          <option value="cookie">A cookie</option>
        </select>
      </FormField>
      {values.hashKind === "client_ip" ? null : (
        <FormField
          label={values.hashKind === "header" ? "Header name" : "Cookie name"}
          htmlFor={nameId}
          error={error("lb.hashKey")}
          hint="Requests without it are spread round robin."
        >
          <input
            id={nameId}
            className="input font-mono"
            value={values.hashName}
            onChange={(event) => onChange({ hashName: event.target.value })}
            autoComplete="off"
            {...errorProps(nameId, error("lb.hashKey"))}
          />
        </FormField>
      )}
    </div>
  );
}

/** Algorithm and failure policy up front; checks, ejection and budget folded away. */
export default function PolicyFields(props: PolicyFieldsProps) {
  const { values, onChange, error, idPrefix, openAdvanced } = props;
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const id = (name: string) => `${idPrefix}-${name}`;
  const algorithm = LB_ALGORITHMS.find((entry) => entry.value === values.algorithm);
  const checking = values.healthCheck !== "none";

  return (
    <>
      <FormField label="Load balancing" htmlFor={id("algorithm")} hint={algorithm?.hint}>
        <select
          id={id("algorithm")}
          className="input"
          value={values.algorithm}
          onChange={(event) => onChange({ algorithm: event.target.value as PolicyValues["algorithm"] })}
        >
          {LB_ALGORITHMS.map((entry) => (
            <option key={entry.value} value={entry.value}>
              {entry.label}
            </option>
          ))}
        </select>
      </FormField>
      {values.algorithm === "consistent_hash" ? <HashKeyFields {...props} /> : null}

      <FormField
        label="When no endpoint is healthy"
        htmlFor={id("failure-policy")}
        hint="Failing open keeps serving, so a wrong health verdict is not a total outage."
      >
        <select
          id={id("failure-policy")}
          className="input"
          value={values.failurePolicy}
          onChange={(event) => onChange({ failurePolicy: event.target.value as PolicyValues["failurePolicy"] })}
        >
          <option value="fail_open">Keep serving (fail open)</option>
          <option value="fail_closed">Answer 503 (fail closed)</option>
        </select>
      </FormField>

      <details
        open={advancedOpen || openAdvanced}
        onToggle={(event) => setAdvancedOpen(event.currentTarget.open)}
        className="rounded-md border border-line px-3 py-2"
      >
        <summary className="cursor-pointer text-[0.88rem] font-medium">
          Advanced: health checks, ejection and retry budget
        </summary>
        <div className="mt-3 flex flex-col gap-4">
          <FormField label="Active health check" htmlFor={id("health-check")}>
            <select
              id={id("health-check")}
              className="input"
              value={values.healthCheck}
              onChange={(event) => onChange({ healthCheck: event.target.value as PolicyValues["healthCheck"] })}
            >
              <option value="none">Off</option>
              <option value="http">HTTP request</option>
              <option value="tcp">TCP connect</option>
            </select>
          </FormField>
          {checking ? (
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              {values.healthCheck === "http" ? (
                <>
                  <FormField label="Probe path" htmlFor={id("health-path")} error={error("health.path")}>
                    <input
                      id={id("health-path")}
                      className="input font-mono"
                      value={values.healthPath}
                      onChange={(event) => onChange({ healthPath: event.target.value })}
                      {...errorProps(id("health-path"), error("health.path"))}
                    />
                  </FormField>
                  <FormField
                    label="Healthy status from"
                    htmlFor={id("expected-min")}
                    error={error("health.expectedStatus")}
                  >
                    <div className="flex items-center gap-2">
                      <input
                        id={id("expected-min")}
                        className="input"
                        inputMode="numeric"
                        value={values.expectedMin}
                        onChange={(event) => onChange({ expectedMin: event.target.value })}
                        {...errorProps(id("expected-min"), error("health.expectedStatus"))}
                      />
                      <label htmlFor={id("expected-max")} className="text-[0.82rem]">
                        to
                      </label>
                      <input
                        id={id("expected-max")}
                        className="input"
                        inputMode="numeric"
                        value={values.expectedMax}
                        onChange={(event) => onChange({ expectedMax: event.target.value })}
                      />
                    </div>
                  </FormField>
                </>
              ) : null}
              <NumberField id={id("interval")} label="Probe interval (ms)" value={values.intervalMs} onChange={(v) => onChange({ intervalMs: v })} error={error("health.intervalMs")} />
              <NumberField id={id("timeout")} label="Probe timeout (ms)" value={values.timeoutMs} onChange={(v) => onChange({ timeoutMs: v })} error={error("health.timeoutMs")} />
              <NumberField id={id("rise")} label="Good probes to mark healthy" value={values.rise} onChange={(v) => onChange({ rise: v })} error={error("health.rise")} />
              <NumberField id={id("fall")} label="Failed probes to mark unhealthy" value={values.fall} onChange={(v) => onChange({ fall: v })} error={error("health.fall")} />
            </div>
          ) : null}

          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <NumberField
              id={id("passive-failures")}
              label="Failures before ejection"
              value={values.passiveFailures}
              onChange={(v) => onChange({ passiveFailures: v })}
              error={error("passive.consecutiveFailures")}
              hint="Consecutive failed requests. 0 turns ejection off."
            />
            <NumberField id={id("max-percent")} label="Never eject more than (%)" value={values.maxEjectionPercent} onChange={(v) => onChange({ maxEjectionPercent: v })} error={error("passive.maxEjectionPercent")} />
            <NumberField id={id("base-ejection")} label="First ejection (ms)" value={values.baseEjectionMs} onChange={(v) => onChange({ baseEjectionMs: v })} error={error("passive.baseEjectionMs")} hint="Doubles each time the endpoint fails again." />
            <NumberField id={id("max-ejection")} label="Longest ejection (ms)" value={values.maxEjectionMs} onChange={(v) => onChange({ maxEjectionMs: v })} error={error("passive.maxEjectionMs")} />
            <NumberField id={id("retry-percent")} label="Retries as % of requests" value={values.retryPercent} onChange={(v) => onChange({ retryPercent: v })} error={error("retryBudget.percent")} hint="Caps retries so a failing group is not hammered." />
            <NumberField id={id("retry-min")} label="Minimum retries per second" value={values.retryMinPerSec} onChange={(v) => onChange({ retryMinPerSec: v })} error={error("retryBudget.minPerSec")} />
          </div>
        </div>
      </details>
    </>
  );
}
