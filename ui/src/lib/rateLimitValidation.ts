// Client-side checks for a rate-limit policy, mirroring the server's rules in
// `fast-proxy-model` (rate_limit.rs). Field names are the server's
// (`rateLimit.rate`, ...), so its errors land on the same inputs.
import { wholeNumber, type FieldErrors, type Validated } from "./policyValidation";
import type { RateLimit, RateLimitInput } from "./types";

export const MAX_RATE = 1_000_000;
export const MAX_PERIOD_SECS = 3_600;
export const MAX_BURST = 10_000;
export const DEFAULT_IPV6_PREFIX = 64;
const MAX_HEADER_NAME_LEN = 64;
const TOKEN = /^[!#$%&'*+\-.^_`|~0-9a-z]+$/;
/** Headers that describe the connection, not the caller (the server refuses them). */
const RESERVED_HEADERS = [
  "connection",
  "content-length",
  "host",
  "keep-alive",
  "proxy-authenticate",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
];

export type PeriodUnit = "s" | "m" | "h";
const UNIT_SECS: Record<PeriodUnit, number> = { s: 1, m: 60, h: 3600 };

export interface RateLimitValues {
  rate: string;
  periodAmount: string;
  periodUnit: PeriodUnit;
  /** Empty: the same as the rate. */
  burst: string;
  key: "client" | "header";
  header: string;
  ipv6Prefix: string;
}

/** The form for a new policy: 10 requests a second per client. */
export function defaultRateLimitValues(): RateLimitValues {
  return {
    rate: "10",
    periodAmount: "1",
    periodUnit: "s",
    burst: "",
    key: "client",
    header: "",
    ipv6Prefix: String(DEFAULT_IPV6_PREFIX),
  };
}

/** The form values of an existing policy, its period in the largest whole unit. */
export function rateLimitValuesFrom(policy: RateLimit): RateLimitValues {
  const unit: PeriodUnit =
    policy.periodSecs % 3600 === 0 ? "h" : policy.periodSecs % 60 === 0 ? "m" : "s";
  return {
    rate: String(policy.rate),
    periodAmount: String(policy.periodSecs / UNIT_SECS[unit]),
    periodUnit: unit,
    burst: policy.burst === policy.rate ? "" : String(policy.burst),
    key: policy.key,
    header: policy.header ?? "",
    ipv6Prefix: String(policy.ipv6Prefix),
  };
}

function bounded(errors: FieldErrors, field: string, raw: string, min: number, max: number): number {
  const value = wholeNumber(raw);
  if (value === null || value < min || value > max) {
    errors[field] = `Must be a whole number from ${min} to ${max}`;
    return min;
  }
  return value;
}

/** Checks the values and builds the `PUT` body, sending only what differs from the server's defaults. */
export function validateRateLimit(values: RateLimitValues): Validated<RateLimitInput> {
  const errors: FieldErrors = {};
  const rate = bounded(errors, "rateLimit.rate", values.rate, 1, MAX_RATE);
  const amount = wholeNumber(values.periodAmount);
  const periodSecs = amount === null ? 0 : amount * UNIT_SECS[values.periodUnit];
  if (periodSecs < 1 || periodSecs > MAX_PERIOD_SECS) {
    errors["rateLimit.periodSecs"] = "Must be from 1 second to 1 hour";
  }
  const input: RateLimitInput = { rate, periodSecs };
  if (values.burst.trim() !== "") {
    input.burst = bounded(errors, "rateLimit.burst", values.burst, 1, MAX_BURST);
  } else if (!errors["rateLimit.rate"] && rate > MAX_BURST) {
    errors["rateLimit.burst"] = `The burst defaults to the rate, which is above ${MAX_BURST}: set it`;
  }
  if (values.key === "header") {
    const header = values.header.trim().toLowerCase();
    if (header === "" || header.length > MAX_HEADER_NAME_LEN || !TOKEN.test(header)) {
      errors["rateLimit.header"] = "Enter a header name, such as X-Api-Key";
    } else if (RESERVED_HEADERS.includes(header)) {
      errors["rateLimit.header"] = `Cannot be ${header}: it describes the connection, not the caller`;
    }
    input.key = "header";
    input.header = header;
  } else {
    const prefix = bounded(errors, "rateLimit.ipv6Prefix", values.ipv6Prefix, 48, 128);
    if (prefix !== DEFAULT_IPV6_PREFIX) input.ipv6Prefix = prefix;
  }
  return Object.keys(errors).length > 0 ? { errors } : { errors, value: input };
}

/** `10 / s, burst 20` or `100 / 5 min by X-Api-Key`. */
export function rateLimitSummary(policy: RateLimit): string {
  const unit =
    policy.periodSecs === 1
      ? "s"
      : policy.periodSecs === 60
        ? "min"
        : policy.periodSecs === 3600
          ? "h"
          : policy.periodSecs % 60 === 0
            ? `${policy.periodSecs / 60} min`
            : `${policy.periodSecs} s`;
  let text = `${policy.rate} / ${unit}`;
  if (policy.burst !== policy.rate) text += `, burst ${policy.burst}`;
  if (policy.key === "header" && policy.header) text += ` by ${policy.header}`;
  return text;
}
