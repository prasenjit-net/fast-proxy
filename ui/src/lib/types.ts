// Types of the admin API. Config responses are `{ data, version }`, where
// `version` is the config revision the response reflects (for a write, the
// one it created).

export interface UiConfig {
  appName: string;
  tagline: string;
  defaultTheme: string;
  repoUrl?: string | null;
}

export interface ServerConfig {
  ui: UiConfig;
  version: string;
  startedAtMs: number;
}

/** Request and error rates over one sampling window. */
export interface TrafficRates {
  requestsTotal: number;
  requestsPerSec: number;
  /** Share of the window's requests that got a 5xx response, 0 to 1. */
  errorRate: number;
}

export interface SubsystemTraffic extends TrafficRates {
  name: string;
}

/** One sampling window of live proxy traffic (the `traffic` push event). */
export interface Traffic {
  timestampMs: number;
  windowSecs: number;
  total: TrafficRates;
  activeConnections: number;
  activeRequests: number;
  subsystems: SubsystemTraffic[];
}

export interface Envelope<T> {
  data: T;
  version: number;
}

export interface VersionOnly {
  version: number;
}

// ---- subsystems -----------------------------------------------------------

export interface SubsystemSummary {
  id: number;
  name: string;
  enabled: boolean;
  description: string;
  hosts: string[];
  routeCount: number;
  updatedAtMs: number;
}

export interface Host {
  id: number;
  pattern: string;
}

/** What the operator has decided about an endpoint. */
export type EndpointState = "enabled" | "draining" | "disabled";

export interface Endpoint {
  id: number;
  addr: string;
  weight: number;
  state: EndpointState;
}

export type LbAlgorithm =
  | "round_robin"
  | "weighted_round_robin"
  | "least_connections"
  | "p2c_ewma"
  | "consistent_hash";

export interface LbPolicy {
  algorithm: LbAlgorithm;
  /** `client_ip`, `header:<name>` or `cookie:<name>`; used by consistent hash. */
  hashKey: string;
}

/** What a group does when no endpoint is eligible. */
export type FailurePolicy = "fail_open" | "fail_closed";

export type HealthCheckKind = "none" | "http" | "tcp";

export interface HealthPolicy {
  check: HealthCheckKind;
  path: string;
  /** Lowest and highest HTTP status that counts as healthy. */
  expectedStatus: [number, number];
  intervalMs: number;
  timeoutMs: number;
  rise: number;
  fall: number;
}

export interface PassivePolicy {
  /** Consecutive failures that eject an endpoint; 0 turns ejection off. */
  consecutiveFailures: number;
  baseEjectionMs: number;
  maxEjectionMs: number;
  maxEjectionPercent: number;
}

export interface RetryBudget {
  percent: number;
  minPerSec: number;
}

/** How a group spreads load and protects its endpoints. */
export interface UpstreamPolicy {
  lb: LbPolicy;
  failurePolicy: FailurePolicy;
  health: HealthPolicy;
  passive: PassivePolicy;
  retryBudget: RetryBudget;
}

export interface Upstream extends UpstreamPolicy {
  id: number;
  name: string;
  endpoints: Endpoint[];
}

/** A route's retry behaviour. */
export interface RetryPolicy {
  maxAttempts: number;
  idempotentOn5xx: boolean;
  onTimeout: boolean;
}

export interface Route {
  id: number;
  prefix: string;
  upstreamGroupId: number;
  stripPrefix: boolean;
  upstreamPrefix: string;
  preserveHost: boolean;
  connectTimeoutMs: number;
  responseTimeoutMs: number;
  retry: RetryPolicy;
  enabled: boolean;
}

export interface SubsystemDetail {
  id: number;
  name: string;
  enabled: boolean;
  description: string;
  createdAtMs: number;
  updatedAtMs: number;
  hosts: Host[];
  upstreams: Upstream[];
  /** In match order: longest prefix first. */
  routes: Route[];
}

export interface SubsystemInput {
  name: string;
  enabled: boolean;
  description: string;
}

/** The body of `PUT` on an upstream: its name and policy (endpoints are separate). */
export interface UpstreamSettings extends UpstreamPolicy {
  name: string;
}

/** The body of `POST` and `PUT` on an endpoint. */
export interface EndpointInput {
  addr: string;
  weight: number;
}

/** The body of `POST` on a subsystem's upstreams: settings plus the first endpoint. */
export interface UpstreamCreate extends UpstreamSettings {
  endpoints: EndpointInput[];
}

export type RouteInput = Omit<Route, "id">;

// ---- revisions, resolver, status -------------------------------------------

export interface RevisionSummary {
  version: number;
  createdAtMs: number;
  summary: string;
}

export interface RevisionPage {
  data: RevisionSummary[];
  /** The current config version. */
  version: number;
  /** Pass as `before` for the next (older) page; null on the last page. */
  nextBefore: number | null;
}

export interface SubsystemView {
  id: number;
  name: string;
  enabled: boolean;
  description: string;
  hosts: string[];
  upstreams: Upstream[];
  routes: Route[];
}

export interface RevisionDetail extends RevisionSummary {
  config: { subsystems: SubsystemView[] };
}

export interface Resolution {
  matched: boolean;
  /** `unknown_host`, `subsystem_disabled`, or `no_route` when nothing matched. */
  reason: "unknown_host" | "subsystem_disabled" | "no_route" | null;
  subsystem: { id: number; name: string } | null;
  route: {
    id: number;
    prefix: string;
    upstreamName: string;
    algorithm: string;
    endpoints: {
      id: number;
      addr: string;
      weight: number;
      state: "enabled" | "draining" | "disabled";
      /** Admin-enabled, healthy and not ejected right now. */
      eligible: boolean;
      upstreamUrl: string | null;
    }[];
    stripPrefix: boolean;
    upstreamPrefix: string;
    preserveHost: boolean;
  } | null;
}

/** An endpoint that is not taking requests, and why. */
export interface UnhealthyEndpoint {
  subsystem: string;
  upstream: string;
  endpoint: string;
  reason: string;
}

/** How many endpoints are in rotation across all upstreams. */
export interface UpstreamHealth {
  total: number;
  healthy: number;
  unhealthy: UnhealthyEndpoint[];
}

/** What the proxy does with an endpoint right now. */
export type EffectiveState = "healthy" | "unhealthy" | "ejected" | "draining" | "disabled";

export interface EndpointStatus {
  id: number;
  addr: string;
  weight: number;
  state: EndpointState;
  effective: EffectiveState;
  reason: string;
  inFlight: number;
  /** Smoothed time to first byte; null before the first response. */
  ewmaMs: number | null;
  lastProbe: { ok: boolean; agoMs: number } | null;
}

/** The live state of an upstream group. */
export interface UpstreamStatus {
  name: string;
  algorithm: LbAlgorithm;
  failurePolicy: FailurePolicy;
  healthy: number;
  endpoints: EndpointStatus[];
}

export interface Status {
  listeners: { name: string; addr: string }[];
  database: string;
  subsystems: { total: number; enabled: number };
  upstreams: UpstreamHealth;
  startedAtMs: number;
  uptimeSecs: number;
}
