// Cheap client-side checks that mirror the server's rules, so obvious
// mistakes are caught before a round trip. The server stays the authority:
// it re-validates everything and its field errors are shown the same way.
// Field names are the API's camelCase names, shared with server errors.
import type { RouteInput, SubsystemInput, UpstreamInput } from "./types";

export type FieldErrors = Record<string, string>;

/** The outcome of validating a form: errors, or the request body to send. */
export type Validated<T> = { errors: FieldErrors; value?: undefined } | { errors: FieldErrors; value: T };

const SUBSYSTEM_NAME = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;
const MIN_TIMEOUT_MS = 100;
const MAX_TIMEOUT_MS = 600_000;

function done<T>(errors: FieldErrors, value: T): Validated<T> {
  return Object.keys(errors).length > 0 ? { errors } : { errors, value };
}

/** A whole number written as digits, or null. */
function wholeNumber(raw: string): number | null {
  return /^\d+$/.test(raw.trim()) ? Number(raw.trim()) : null;
}

export interface SubsystemValues {
  name: string;
  description: string;
  enabled: boolean;
}

export function validateSubsystem(values: SubsystemValues): Validated<SubsystemInput> {
  const errors: FieldErrors = {};
  const name = values.name.trim();
  if (!SUBSYSTEM_NAME.test(name)) {
    errors.name =
      "Use 1 to 63 lowercase letters, digits or '-', starting and ending with a letter or digit";
  }
  if (values.description.length > 500) {
    errors.description = "Must be at most 500 characters";
  }
  return done(errors, { name, description: values.description, enabled: values.enabled });
}

export function validateHost(pattern: string): Validated<string> {
  const errors: FieldErrors = {};
  const value = pattern.trim();
  if (value === "") {
    errors.pattern = "Enter a host name, such as shop.example.com or *.example.com";
  } else if (/[\s/:]/.test(value)) {
    errors.pattern = "Enter only the host name: no spaces, scheme, port or path";
  }
  return done(errors, value);
}

export interface UpstreamValues {
  name: string;
  addr: string;
  weight: string;
}

export function validateUpstream(values: UpstreamValues): Validated<UpstreamInput> {
  const errors: FieldErrors = {};
  const name = values.name.trim();
  if (name === "" || name.length > 63) errors.name = "Must be 1 to 63 characters";

  const addr = values.addr.trim();
  const port = addr.includes(":") ? wholeNumber(addr.slice(addr.lastIndexOf(":") + 1)) : null;
  if (port === null || port < 1 || port > 65535 || addr.startsWith(":")) {
    errors.addr = "Use host:port, with a port from 1 to 65535";
  }

  const weight = wholeNumber(values.weight);
  if (weight === null || weight < 1) errors.weight = "Must be a whole number, at least 1";

  return done(errors, { name, addr, weight: weight ?? 1 });
}

export interface RouteValues {
  prefix: string;
  upstreamGroupId: string;
  stripPrefix: boolean;
  upstreamPrefix: string;
  preserveHost: boolean;
  connectTimeoutMs: string;
  responseTimeoutMs: string;
  enabled: boolean;
}

function timeout(raw: string): number | null {
  const value = wholeNumber(raw);
  return value !== null && value >= MIN_TIMEOUT_MS && value <= MAX_TIMEOUT_MS ? value : null;
}

function pathProblem(path: string): string | null {
  if (!path.startsWith("/")) return "Must start with '/'";
  if (/[\s?#\\]/.test(path)) return "Must not contain spaces, '?', '#' or '\\'";
  if (path.includes("//")) return "Must not contain '//'";
  if (path.split("/").some((segment) => segment === "." || segment === "..")) {
    return "Must not contain '.' or '..' segments";
  }
  return null;
}

export function validateRoute(values: RouteValues): Validated<RouteInput> {
  const errors: FieldErrors = {};

  const prefix = values.prefix.trim();
  const prefixProblem = pathProblem(prefix);
  if (prefixProblem) errors.prefix = prefixProblem;

  const upstreamPrefix = values.upstreamPrefix.trim();
  if (upstreamPrefix !== "") {
    const problem = pathProblem(upstreamPrefix);
    if (problem) errors.upstreamPrefix = problem;
  }

  const upstreamGroupId = wholeNumber(values.upstreamGroupId);
  if (upstreamGroupId === null) errors.upstreamGroupId = "Choose an upstream";

  const connectTimeoutMs = timeout(values.connectTimeoutMs);
  if (connectTimeoutMs === null) errors.connectTimeoutMs = "Must be between 100 and 600000 ms";
  const responseTimeoutMs = timeout(values.responseTimeoutMs);
  if (responseTimeoutMs === null) errors.responseTimeoutMs = "Must be between 100 and 600000 ms";

  return done(errors, {
    prefix,
    upstreamGroupId: upstreamGroupId ?? 0,
    stripPrefix: values.stripPrefix,
    upstreamPrefix,
    preserveHost: values.preserveHost,
    connectTimeoutMs: connectTimeoutMs ?? 0,
    responseTimeoutMs: responseTimeoutMs ?? 0,
    enabled: values.enabled,
  });
}
