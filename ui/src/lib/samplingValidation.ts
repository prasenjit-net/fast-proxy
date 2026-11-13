// Form values and client-side checks for the sampling settings and an override.
// They mirror the server's bounds (the model crate); field names are the API's
// camelCase names, shared with server errors. The server re-validates.
import type { FieldErrors, Validated } from "./policyValidation";
import type {
  SampleTrigger,
  SamplingOverride,
  SamplingOverrideInput,
  SamplingSettings,
  SamplingSettingsInput,
} from "./types";

const MIB = 1024 * 1024;
const KIB = 1024;

/** The triggers, in the order the page lists them, each with what it means. */
export const TRIGGERS: { value: SampleTrigger; label: string; hint: string }[] = [
  {
    value: "upstream_failure",
    label: "Upstream unreachable",
    hint: "A connection, TLS or timeout failure ended the request (the proxy's own 502 or 504). An upstream's own 5xx is never sampled.",
  },
  {
    value: "retried",
    label: "Recovered by a retry",
    hint: "A connectivity failure was retried and the request then got an answer.",
  },
  { value: "slow", label: "Slow", hint: "Took longer than the threshold below." },
  {
    value: "slow_p99",
    label: "Slower than the route's usual",
    hint: "Took several times the route's recent 99th percentile. Needs a few hundred requests to know it.",
  },
  {
    value: "client_errors",
    label: "A client that keeps failing",
    hint: "One client address earned many 4xx responses in a minute.",
  },
  { value: "oversized_request", label: "Large request", hint: "The request body reached the size below." },
  { value: "oversized_response", label: "Large response", hint: "The response body reached the size below." },
  {
    value: "unusual_headers",
    label: "Unusual headers",
    hint: "Very many or very long headers, odd bytes, a repeated Host, or an unknown method.",
  },
  { value: "baseline", label: "Random baseline", hint: "A small share of ordinary requests, for comparison." },
];

/** Everything the settings form edits, as the strings typed. */
export interface SamplingValues {
  enabled: boolean;
  triggers: SampleTrigger[];
  slowThresholdMs: string;
  /** The factor as written, such as 3 or 2.5 (the API takes it times ten). */
  slowP99Factor: string;
  slowP99FloorMs: string;
  clientErrorsPerMin: string;
  oversizedRequestMiB: string;
  oversizedResponseMiB: string;
  baselinePerMille: string;
  ratePerSec: string;
  rateBurst: string;
  stormBurst: string;
  stormIntervalSecs: string;
  retentionDays: string;
  maxRows: string;
  maxMiB: string;
  bodyRequestKiB: string;
  bodyResponseKiB: string;
  /** One name per line. */
  extraHeaders: string;
  redactHeaders: string;
  redactQueryParams: string;
  redactBodyFields: string;
}

/** The names of a list typed one per line; blank lines are ignored. */
export function parseList(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

export function listText(names: string[]): string {
  return names.join("\n");
}

export function valuesOf(settings: SamplingSettings): SamplingValues {
  const text = (n: number) => String(n);
  return {
    enabled: settings.enabled,
    triggers: settings.triggers,
    slowThresholdMs: text(settings.slowThresholdMs),
    slowP99Factor: text(settings.slowP99FactorX10 / 10),
    slowP99FloorMs: text(settings.slowP99FloorMs),
    clientErrorsPerMin: text(settings.clientErrorsPerMin),
    oversizedRequestMiB: text(settings.oversizedRequestBytes / MIB),
    oversizedResponseMiB: text(settings.oversizedResponseBytes / MIB),
    baselinePerMille: text(settings.baselinePerMille),
    ratePerSec: text(settings.ratePerSec),
    rateBurst: text(settings.rateBurst),
    stormBurst: text(settings.stormBurst),
    stormIntervalSecs: text(settings.stormIntervalSecs),
    retentionDays: text(settings.retentionDays),
    maxRows: text(settings.maxRows),
    maxMiB: text(settings.maxBytes / MIB),
    bodyRequestKiB: text(settings.bodyRequestCap / KIB),
    bodyResponseKiB: text(settings.bodyResponseCap / KIB),
    extraHeaders: listText(settings.extraHeaders),
    redactHeaders: listText(settings.redactHeaders),
    redactQueryParams: listText(settings.redactQueryParams),
    redactBodyFields: listText(settings.redactBodyFields),
  };
}

/** A whole number in `[min, max]`, or an error message. */
function whole(raw: string, min: number, max: number, errors: FieldErrors, field: string): number {
  const value = Number(raw);
  if (raw.trim() === "" || !Number.isInteger(value) || value < min || value > max) {
    errors[field] = `Use a whole number from ${min.toLocaleString()} to ${max.toLocaleString()}`;
  }
  return value;
}

const HEADER_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]{1,64}$/;

function checkNames(text: string, kind: "header" | "plain", errors: FieldErrors, field: string): string[] {
  const names = parseList(text);
  if (names.length > 64) {
    errors[field] = "At most 64 names";
  } else if (kind === "header" && names.some((name) => !HEADER_NAME.test(name))) {
    errors[field] = "Each line must be a header name of at most 64 characters, such as X-Tenant";
  } else if (
    kind === "plain" &&
    names.some((name) => name.length > 64 || /[\s=&"\u0000-\u001f]/.test(name))
  ) {
    errors[field] = "Each line must be a name of at most 64 characters without spaces, = or &";
  }
  return names;
}

export function validateSampling(values: SamplingValues): Validated<SamplingSettingsInput> {
  const errors: FieldErrors = {};
  const factor = Number(values.slowP99Factor);
  const factorX10 = Math.round(factor * 10);
  if (values.slowP99Factor.trim() === "" || !Number.isFinite(factor) || factorX10 < 11 || factorX10 > 1000) {
    errors.slowP99FactorX10 = "Use a factor from 1.1 to 100, such as 3";
  }
  const input: SamplingSettingsInput = {
    enabled: values.enabled,
    triggers: values.triggers,
    slowThresholdMs: whole(values.slowThresholdMs, 1, 3_600_000, errors, "slowThresholdMs"),
    slowP99FactorX10: factorX10,
    slowP99FloorMs: whole(values.slowP99FloorMs, 1, 3_600_000, errors, "slowP99FloorMs"),
    clientErrorsPerMin: whole(values.clientErrorsPerMin, 1, 1_000_000, errors, "clientErrorsPerMin"),
    oversizedRequestBytes:
      whole(values.oversizedRequestMiB, 1, 65_536, errors, "oversizedRequestBytes") * MIB,
    oversizedResponseBytes:
      whole(values.oversizedResponseMiB, 1, 65_536, errors, "oversizedResponseBytes") * MIB,
    baselinePerMille: whole(values.baselinePerMille, 0, 1000, errors, "baselinePerMille"),
    ratePerSec: whole(values.ratePerSec, 1, 10_000, errors, "ratePerSec"),
    rateBurst: whole(values.rateBurst, 1, 10_000, errors, "rateBurst"),
    stormBurst: whole(values.stormBurst, 1, 100, errors, "stormBurst"),
    stormIntervalSecs: whole(values.stormIntervalSecs, 1, 3600, errors, "stormIntervalSecs"),
    retentionDays: whole(values.retentionDays, 1, 365, errors, "retentionDays"),
    maxRows: whole(values.maxRows, 100, 10_000_000, errors, "maxRows"),
    maxBytes: whole(values.maxMiB, 1, 16_384, errors, "maxBytes") * MIB,
    bodyRequestCap: whole(values.bodyRequestKiB, 1, 64, errors, "bodyRequestCap") * KIB,
    bodyResponseCap: whole(values.bodyResponseKiB, 1, 64, errors, "bodyResponseCap") * KIB,
    extraHeaders: checkNames(values.extraHeaders, "header", errors, "extraHeaders"),
    redactHeaders: checkNames(values.redactHeaders, "header", errors, "redactHeaders"),
    redactQueryParams: checkNames(values.redactQueryParams, "plain", errors, "redactQueryParams"),
    redactBodyFields: checkNames(values.redactBodyFields, "plain", errors, "redactBodyFields"),
  };
  if (Object.keys(errors).length > 0) return { errors };
  return { errors, value: input };
}

/** What the baseline rate means in words: "about 1 in 1000 requests". */
export function baselineWords(perMille: number): string {
  if (perMille <= 0) return "none of the ordinary requests";
  if (perMille >= 1000) return "every request";
  const every = Math.round(1000 / perMille);
  return `about 1 in ${every.toLocaleString()} ordinary requests`;
}

// ---- an override ------------------------------------------------------------

export type Tri = "inherit" | "on" | "off";

export interface OverrideValues {
  enabled: Tri;
  captureBodies: Tri;
  /** Empty inherits. */
  baselinePerMille: string;
  /** Empty inherits. */
  slowThresholdMs: string;
}

export const NO_OVERRIDE: OverrideValues = {
  enabled: "inherit",
  captureBodies: "inherit",
  baselinePerMille: "",
  slowThresholdMs: "",
};

const triOf = (value: boolean | null): Tri => (value === null ? "inherit" : value ? "on" : "off");
const boolOf = (value: Tri): boolean | undefined => (value === "inherit" ? undefined : value === "on");

export function overrideValuesOf(override: SamplingOverride | undefined): OverrideValues {
  if (!override) return NO_OVERRIDE;
  return {
    enabled: triOf(override.enabled),
    captureBodies: triOf(override.captureBodies),
    baselinePerMille: override.baselinePerMille === null ? "" : String(override.baselinePerMille),
    slowThresholdMs: override.slowThresholdMs === null ? "" : String(override.slowThresholdMs),
  };
}

/** Whether nothing is overridden (so there is nothing to store). */
export function isInherited(values: OverrideValues): boolean {
  return (
    values.enabled === "inherit" &&
    values.captureBodies === "inherit" &&
    values.baselinePerMille.trim() === "" &&
    values.slowThresholdMs.trim() === ""
  );
}

export function validateOverride(
  values: OverrideValues,
  target: { subsystemId: number } | { routeId: number },
): Validated<SamplingOverrideInput> {
  const errors: FieldErrors = {};
  const input: SamplingOverrideInput = { ...target };
  const enabled = boolOf(values.enabled);
  if (enabled !== undefined) input.enabled = enabled;
  const captureBodies = boolOf(values.captureBodies);
  if (captureBodies !== undefined) input.captureBodies = captureBodies;
  if (values.baselinePerMille.trim() !== "") {
    input.baselinePerMille = whole(values.baselinePerMille, 0, 1000, errors, "baselinePerMille");
  }
  if (values.slowThresholdMs.trim() !== "") {
    input.slowThresholdMs = whole(values.slowThresholdMs, 1, 3_600_000, errors, "slowThresholdMs");
  }
  if (Object.keys(errors).length > 0) return { errors };
  return { errors, value: input };
}

/** A few words for what an override changes, for a badge. */
export function overrideSummary(override: SamplingOverride | undefined): string {
  if (!override) return "inherits";
  const parts: string[] = [];
  if (override.enabled !== null) parts.push(override.enabled ? "on" : "off");
  if (override.captureBodies !== null) parts.push(override.captureBodies ? "bodies on" : "bodies off");
  if (override.baselinePerMille !== null) parts.push(`baseline ${override.baselinePerMille}‰`);
  if (override.slowThresholdMs !== null) parts.push(`slow ${override.slowThresholdMs} ms`);
  return parts.length > 0 ? parts.join(", ") : "inherits";
}
