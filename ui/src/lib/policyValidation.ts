// Form values and client-side checks for an upstream group's policy and its
// endpoints. They mirror the server's bounds (see the model crate); field
// names are the API's dotted camelCase paths, shared with server errors.
import type {
  EndpointInput,
  FailurePolicy,
  HealthCheckKind,
  LbAlgorithm,
  UpstreamPolicy,
} from "./types";

export type FieldErrors = Record<string, string>;
export type Validated<T> = { errors: FieldErrors; value?: undefined } | { errors: FieldErrors; value: T };

export const LB_ALGORITHMS: { value: LbAlgorithm; label: string; hint: string }[] = [
  { value: "round_robin", label: "Round robin", hint: "Each endpoint in turn." },
  { value: "weighted_round_robin", label: "Weighted round robin", hint: "In turn, in proportion to weight." },
  { value: "least_connections", label: "Least connections", hint: "The endpoint with the fewest requests in flight." },
  { value: "p2c_ewma", label: "Lowest latency (power of two)", hint: "Prefers faster, less busy endpoints." },
  { value: "consistent_hash", label: "Consistent hash", hint: "The same client or key reaches the same endpoint." },
];

export type HashKind = "client_ip" | "header" | "cookie";

/** Everything the policy part of the form edits, as the strings typed. */
export interface PolicyValues {
  algorithm: LbAlgorithm;
  hashKind: HashKind;
  hashName: string;
  failurePolicy: FailurePolicy;
  healthCheck: HealthCheckKind;
  healthPath: string;
  expectedMin: string;
  expectedMax: string;
  intervalMs: string;
  timeoutMs: string;
  rise: string;
  fall: string;
  passiveFailures: string;
  baseEjectionMs: string;
  maxEjectionMs: string;
  maxEjectionPercent: string;
  retryPercent: string;
  retryMinPerSec: string;
  tlsEnabled: boolean;
  tlsVerify: boolean;
  tlsServerName: string;
  tlsCa: string;
}

/** The server's defaults, so a new group works with no further input. */
export function defaultPolicyValues(): PolicyValues {
  return {
    algorithm: "round_robin",
    hashKind: "client_ip",
    hashName: "",
    failurePolicy: "fail_open",
    healthCheck: "none",
    healthPath: "/",
    expectedMin: "200",
    expectedMax: "399",
    intervalMs: "5000",
    timeoutMs: "2000",
    rise: "2",
    fall: "3",
    passiveFailures: "3",
    baseEjectionMs: "10000",
    maxEjectionMs: "300000",
    maxEjectionPercent: "50",
    retryPercent: "20",
    retryMinPerSec: "10",
    tlsEnabled: false,
    tlsVerify: true,
    tlsServerName: "",
    tlsCa: "",
  };
}

/** The form values for an existing group's policy. */
export function policyValuesFrom(policy: UpstreamPolicy): PolicyValues {
  const [hashKind, hashName] = splitHashKey(policy.lb.hashKey);
  return {
    algorithm: policy.lb.algorithm,
    hashKind,
    hashName,
    failurePolicy: policy.failurePolicy,
    healthCheck: policy.health.check,
    healthPath: policy.health.path,
    expectedMin: String(policy.health.expectedStatus[0]),
    expectedMax: String(policy.health.expectedStatus[1]),
    intervalMs: String(policy.health.intervalMs),
    timeoutMs: String(policy.health.timeoutMs),
    rise: String(policy.health.rise),
    fall: String(policy.health.fall),
    passiveFailures: String(policy.passive.consecutiveFailures),
    baseEjectionMs: String(policy.passive.baseEjectionMs),
    maxEjectionMs: String(policy.passive.maxEjectionMs),
    maxEjectionPercent: String(policy.passive.maxEjectionPercent),
    retryPercent: String(policy.retryBudget.percent),
    retryMinPerSec: String(policy.retryBudget.minPerSec),
    tlsEnabled: policy.tls.enabled,
    tlsVerify: policy.tls.verify,
    tlsServerName: policy.tls.serverName,
    tlsCa: policy.tls.ca,
  };
}

function splitHashKey(key: string): [HashKind, string] {
  const [kind, ...rest] = key.split(":");
  if (kind === "header" || kind === "cookie") return [kind, rest.join(":")];
  return ["client_ip", ""];
}

/** A whole number written as digits, or null. */
export function wholeNumber(raw: string): number | null {
  return /^\d+$/.test(raw.trim()) ? Number(raw.trim()) : null;
}

/** Records an error for `field` unless `raw` is a whole number in range. */
function inRange(errors: FieldErrors, field: string, raw: string, min: number, max: number): number {
  const value = wholeNumber(raw);
  if (value === null || value < min || value > max) {
    errors[field] = `Must be a whole number from ${min} to ${max}`;
    return min;
  }
  return value;
}

const MS = (low: number, high: number) => [low, high] as const;
const INTERVAL = MS(100, 600_000);
const EJECTION = MS(100, 3_600_000);
const HASH_NAME = /^[A-Za-z0-9._-]{1,64}$/;

function validateHealth(values: PolicyValues, errors: FieldErrors): UpstreamPolicy["health"] {
  if (values.healthCheck === "none") {
    const d = defaultPolicyValues();
    return {
      check: "none",
      path: d.healthPath,
      expectedStatus: [200, 399],
      intervalMs: 5000,
      timeoutMs: 2000,
      rise: 2,
      fall: 3,
    };
  }
  const path = values.healthPath.trim();
  if (values.healthCheck === "http" && (!path.startsWith("/") || /\s/.test(path) || path.length > 512)) {
    errors["health.path"] = "Must start with / and contain no spaces";
  }
  const min = inRange(errors, "health.expectedStatus", values.expectedMin, 100, 599);
  const max = inRange(errors, "health.expectedStatus", values.expectedMax, 100, 599);
  if (!errors["health.expectedStatus"] && min > max) {
    errors["health.expectedStatus"] = "The lowest status must not exceed the highest";
  }
  const intervalMs = inRange(errors, "health.intervalMs", values.intervalMs, ...INTERVAL);
  const timeoutMs = inRange(errors, "health.timeoutMs", values.timeoutMs, ...INTERVAL);
  if (!errors["health.timeoutMs"] && !errors["health.intervalMs"] && timeoutMs > intervalMs) {
    errors["health.timeoutMs"] = "Must not exceed the interval";
  }
  return {
    check: values.healthCheck,
    path: path || "/",
    expectedStatus: [min, max],
    intervalMs,
    timeoutMs,
    rise: inRange(errors, "health.rise", values.rise, 1, 10),
    fall: inRange(errors, "health.fall", values.fall, 1, 10),
  };
}

function validatePassive(values: PolicyValues, errors: FieldErrors): UpstreamPolicy["passive"] {
  const baseEjectionMs = inRange(errors, "passive.baseEjectionMs", values.baseEjectionMs, ...EJECTION);
  const maxEjectionMs = inRange(errors, "passive.maxEjectionMs", values.maxEjectionMs, ...EJECTION);
  if (!errors["passive.baseEjectionMs"] && !errors["passive.maxEjectionMs"] && maxEjectionMs < baseEjectionMs) {
    errors["passive.maxEjectionMs"] = "Must not be shorter than the base ejection";
  }
  return {
    consecutiveFailures: inRange(errors, "passive.consecutiveFailures", values.passiveFailures, 0, 100),
    baseEjectionMs,
    maxEjectionMs,
    maxEjectionPercent: inRange(errors, "passive.maxEjectionPercent", values.maxEjectionPercent, 1, 100),
  };
}

const SERVER_NAME = /^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$/;

/** The upstream TLS part: an override name, when given, must be a plain DNS name. */
function validateUpstreamTls(values: PolicyValues, errors: FieldErrors): UpstreamPolicy["tls"] {
  const serverName = values.tlsServerName.trim();
  if (serverName !== "" && (serverName.length > 253 || !SERVER_NAME.test(serverName))) {
    errors["tls.serverName"] = "Enter a host name such as backend.internal (no port, scheme or spaces)";
  }
  return {
    enabled: values.tlsEnabled,
    verify: values.tlsVerify,
    serverName,
    ca: values.tlsCa,
  };
}

/** Checks the policy values and builds the policy part of the request. */
export function validatePolicy(values: PolicyValues): Validated<UpstreamPolicy> {
  const errors: FieldErrors = {};
  let hashKey = "client_ip";
  if (values.algorithm === "consistent_hash" && values.hashKind !== "client_ip") {
    const name = values.hashName.trim();
    if (!HASH_NAME.test(name)) {
      errors["lb.hashKey"] = "Use 1 to 64 letters, digits or - _ .";
    }
    hashKey = `${values.hashKind}:${name}`;
  }
  const policy: UpstreamPolicy = {
    lb: { algorithm: values.algorithm, hashKey },
    failurePolicy: values.failurePolicy,
    health: validateHealth(values, errors),
    passive: validatePassive(values, errors),
    retryBudget: {
      percent: inRange(errors, "retryBudget.percent", values.retryPercent, 0, 100),
      minPerSec: inRange(errors, "retryBudget.minPerSec", values.retryMinPerSec, 0, 1000),
    },
    tls: validateUpstreamTls(values, errors),
  };
  return Object.keys(errors).length > 0 ? { errors } : { errors, value: policy };
}

export interface EndpointValues {
  addr: string;
  weight: string;
}

/** Checks one endpoint; `prefix` namespaces errors (for example `endpoints.0.`). */
export function validateEndpoint(values: EndpointValues, prefix = ""): Validated<EndpointInput> {
  const errors: FieldErrors = {};
  const addr = values.addr.trim();
  const port = addr.includes(":") ? wholeNumber(addr.slice(addr.lastIndexOf(":") + 1)) : null;
  if (port === null || port < 1 || port > 65535 || addr.startsWith(":")) {
    errors[`${prefix}addr`] = "Use host:port, with a port from 1 to 65535";
  }
  const weight = wholeNumber(values.weight);
  if (weight === null || weight < 1 || weight > 1000) {
    errors[`${prefix}weight`] = "Must be a whole number from 1 to 1000";
  }
  return Object.keys(errors).length > 0
    ? { errors }
    : { errors, value: { addr, weight: weight ?? 1 } };
}
