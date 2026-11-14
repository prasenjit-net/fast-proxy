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
  /** What the sampler kept and lost in the window. */
  samples: SampleRates;
  subsystems: SubsystemTraffic[];
}

export interface SampleRates {
  capturedTotal: number;
  capturedPerSec: number;
  droppedPerSec: number;
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

/** How the proxy talks TLS to a group's endpoints. */
export interface UpstreamTls {
  enabled: boolean;
  /** Verify the endpoint's certificate; off is allowed but risky. */
  verify: boolean;
  /** Overrides the name used for SNI and the certificate check. */
  serverName: string;
  /** The name of an uploaded CA bundle; empty means the OS trust store. */
  ca: string;
}

/** How a group spreads load and protects its endpoints. */
export interface UpstreamPolicy {
  lb: LbPolicy;
  failurePolicy: FailurePolicy;
  health: HealthPolicy;
  passive: PassivePolicy;
  retryBudget: RetryBudget;
  tls: UpstreamTls;
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
  https: HttpsSettings;
  hosts: Host[];
  upstreams: Upstream[];
  /** In match order: longest prefix first. */
  routes: Route[];
}

export interface SubsystemInput {
  name: string;
  enabled: boolean;
  description: string;
  /** Left out, the stored HTTPS settings are kept. */
  https?: HttpsInput;
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
  https: HttpsSettings;
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
    /** `https` when the group has TLS on: how the proxy reaches its endpoints. */
    upstreamScheme: "http" | "https";
  } | null;
  /** How HTTPS applies to the matched subsystem; null for an unknown host. */
  https: {
    servedOverHttps: boolean;
    /** What a plain-HTTP request for this host would be redirected to. */
    plainHttpRedirect: { status: number; location: string } | null;
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

/** The soonest-expiring readable certificate. */
export interface SoonestExpiry {
  name: string;
  notAfterMs: number;
  secondsLeft: number;
}

/** The process TLS state and the health of the certificates, from `/api/status`. */
export interface StatusTls {
  enabled: boolean;
  listen: string | null;
  vault: VaultState;
  certificates: number;
  unreadable: number;
  soonestExpiry: SoonestExpiry | null;
}

/** How the admin plane is protected, from `/api/status`. */
export interface AdminStatus {
  auth: "required" | "off";
  protectedChannel: boolean;
  /** The stored certificate the admin plane serves over TLS; null on plain HTTP. */
  tlsCertificate: string | null;
  activeTokens: number;
  activeMaintenance: number;
}

export interface Status {
  admin: AdminStatus;
  tls: StatusTls;
  listeners: { name: string; addr: string }[];
  database: string;
  subsystems: { total: number; enabled: number };
  upstreams: UpstreamHealth;
  startedAtMs: number;
  uptimeSecs: number;
}

// ---- TLS: HTTPS settings, certificates, CA bundles -------------------------

export type RedirectMode = "off" | "301" | "308";

export interface HstsSettings {
  enabled: boolean;
  maxAgeSecs: number;
  includeSubdomains: boolean;
}

/** The certificate attached to a subsystem, as shown beside its settings. */
export interface AttachedCertificate {
  name: string;
  source: CertificateSource;
  notAfterMs: number;
  /** Host patterns of the subsystem the certificate does not cover. */
  uncoveredHosts: string[];
}

/** A subsystem's HTTPS settings as the server reports them. */
export interface HttpsSettings {
  enabled: boolean;
  certificateId: number | null;
  redirect: RedirectMode;
  hsts: HstsSettings;
  /** HTTPS is enabled here and TLS is on for the process; otherwise inert. */
  effective: boolean;
  certificate: AttachedCertificate | null;
}

/** What is sent to change a subsystem's HTTPS settings. */
export interface HttpsInput {
  enabled: boolean;
  certificateId: number | null;
  redirect: RedirectMode;
  hsts: HstsSettings;
}

export type CertificateSource = "uploaded" | "self_signed" | "acme";
export type CertificateState = "ok" | "expiring" | "expired" | "unreadable";
export type VaultState = "ok" | "key mismatch" | "not created";

/** A certificate's public facts. Private keys are write-only: the server never returns one. */
export interface Certificate {
  id: number;
  name: string;
  source: CertificateSource;
  names: string[];
  subject: string;
  issuer: string;
  notBeforeMs: number;
  notAfterMs: number;
  secondsLeft: number;
  state: CertificateState;
  fingerprint: string;
  /** Ids of the subsystems using it. */
  usedBy: number[];
  updatedAtMs: number;
  /** Findings of the write that produced this response, such as expiring soon. */
  warnings?: string[];
}

/** The body of an upload: the key is sent once and never kept. */
export interface CertificateUpload {
  name: string;
  certPem: string;
  keyPem: string;
}

/** Changes to a stored certificate; absent fields keep the stored value. */
export interface CertificateChange {
  name?: string;
  certPem?: string;
  keyPem?: string;
}

export interface CaBundle {
  id: number;
  name: string;
  certCount: number;
  certificates: { subject: string; notAfterMs: number }[];
  updatedAtMs: number;
}

/** `GET /api/tls`: the process-wide TLS switch and whether stored keys are readable. */
export interface TlsStatus {
  enabled: boolean;
  listen: string | null;
  http2: boolean;
  minVersion: string;
  vault: VaultState;
}

// ---- Sign-in, tokens and the audit log -------------------------------------

export type Role = "admin" | "viewer";

/** Who the server says the caller is (`GET /api/whoami`). */
export interface WhoAmI {
  name: string;
  role: Role;
  tokenExpiresAtMs: number | null;
  /** Whether private keys may be sent over this channel. */
  protectedChannel: boolean;
  auth: "required" | "off";
}

/** A token as listed: never its secret. */
export interface Token {
  id: number;
  name: string;
  role: Role;
  createdAtMs: number;
  createdBy: string;
  expiresAtMs: number | null;
  revokedAtMs: number | null;
  lastUsedAtMs: number | null;
}

/** The response to creating a token: the only time its secret is shown. */
export interface CreatedToken extends Token {
  secret: string;
}

export interface TokenInput {
  name: string;
  role: Role;
  expiresInDays?: number;
}

export type AuditOutcome = "ok" | "denied" | "failed";

export interface AuditEntry {
  id: number;
  atMs: number;
  instance: string;
  actor: string;
  role: string;
  source: string;
  requestId: string;
  action: string;
  target: string;
  outcome: AuditOutcome;
  /** Structured detail; text when it was not JSON. */
  detail: unknown;
  revision: number | null;
}

export interface AuditPage {
  data: AuditEntry[];
  /** Pass as `before` for the next, older page; null on the last page. */
  nextBefore: number | null;
  version: number;
}

export interface AuditFilters {
  actor?: string;
  action?: string;
  outcome?: AuditOutcome;
  sinceMs?: number;
}

// ---- Maintenance mode -------------------------------------------------------

export type MaintenanceScopeKind = "global" | "subsystem" | "route";

/** One scope that is in maintenance, as `GET /api/maintenance` lists it. */
export interface MaintenanceScope {
  scope: MaintenanceScopeKind;
  subsystemId: number | null;
  subsystem: string | null;
  routeId: number | null;
  route: string | null;
  message: string;
  contentType: string | null;
  hasCustomBody: boolean;
  retryAfterSecs: number | null;
  /** When it ends by itself; null means until turned off. */
  expiresAtMs: number | null;
  secondsLeft: number | null;
  setBy: string;
  setAtMs: number;
  bypass: { cidrs: string[]; header: string | null; secretSet: boolean };
}

/** The body of a turn-on request; fields left out are not sent. */
export interface MaintenanceInput {
  message: string;
  body?: string;
  contentType?: string;
  retryAfterSecs?: number;
  ttlSecs?: number;
  bypass: { cidrs: string[]; header?: string; secret?: string };
}

/** The answer to turning maintenance on; the secret, when one was generated, is shown once. */
export interface MaintenanceSet {
  data: MaintenanceScope;
  version: number;
  bypassSecret: string | null;
}

/** What a maintenance control acts on. */
export type MaintenanceTarget =
  | { scope: "global" }
  | { scope: "subsystem"; subsystemId: number; name: string }
  | { scope: "route"; subsystemId: number; routeId: number; prefix: string };

// ---- Request sampling -------------------------------------------------------

export type SampleTrigger =
  | "upstream_failure"
  | "retried"
  | "slow"
  | "slow_p99"
  | "client_errors"
  | "oversized_request"
  | "oversized_response"
  | "unusual_headers"
  | "baseline";

/** The names redacted whatever the settings say; shown read-only. */
export interface BuiltInRedaction {
  headers: string[];
  queryParams: string[];
  bodyFields: string[];
}

/** Every setting of `PUT /api/sampling` (all are required: a replace, not a patch). */
export interface SamplingSettingsInput {
  enabled: boolean;
  triggers: SampleTrigger[];
  slowThresholdMs: number;
  slowP99FactorX10: number;
  slowP99FloorMs: number;
  clientErrorsPerMin: number;
  oversizedRequestBytes: number;
  oversizedResponseBytes: number;
  baselinePerMille: number;
  ratePerSec: number;
  rateBurst: number;
  stormBurst: number;
  stormIntervalSecs: number;
  retentionDays: number;
  maxRows: number;
  maxBytes: number;
  bodyRequestCap: number;
  bodyResponseCap: number;
  extraHeaders: string[];
  redactHeaders: string[];
  redactQueryParams: string[];
  redactBodyFields: string[];
}

export interface SamplingSettings extends SamplingSettingsInput {
  builtInRedaction: BuiltInRedaction;
  updatedBy: string;
  updatedAtMs: number;
}

/** What a subsystem's or a route's override changes; null inherits. */
export interface SamplingOverride {
  scope: "subsystem" | "route";
  subsystemId: number | null;
  subsystem: string | null;
  routeId: number | null;
  route: string | null;
  enabled: boolean | null;
  captureBodies: boolean | null;
  baselinePerMille: number | null;
  slowThresholdMs: number | null;
  slowP99FactorX10: number | null;
  setBy: string;
  setAtMs: number;
}

/** The body of `PUT /api/sampling/overrides`: one target, and the fields to override. */
export interface SamplingOverrideInput {
  subsystemId?: number;
  routeId?: number;
  enabled?: boolean;
  captureBodies?: boolean;
  baselinePerMille?: number;
  slowThresholdMs?: number;
  slowP99FactorX10?: number;
}

/** What a viewer may see of sampling: counts, never a sample. */
export interface SamplingStatus {
  /** Whether this process samples (`[sampling].enabled`, a restart to change). */
  processEnabled: boolean;
  captured: number;
  dropped: number;
  /** The shared table; null when it could not be read just now. */
  table: { rows: number; bytes: number; oldestAtMs: number | null } | null;
}

export interface SamplingView {
  settings: SamplingSettings;
  overrides: SamplingOverride[];
  status: SamplingStatus;
}

/** One sample in a list: the short columns, never the detail. */
export interface SampleSummary {
  id: number;
  atMs: number;
  instance: string;
  requestId: string;
  triggers: SampleTrigger[];
  primary: SampleTrigger;
  subsystem: string;
  route: string;
  client: string;
  method: string;
  status: number;
  totalMs: number;
  sizeBytes: number;
}

export interface SamplePage {
  data: SampleSummary[];
  /** Pass as `before` for the next, older page; null on the last page. */
  nextBefore: number | null;
  version: number;
}

export interface SampleBody {
  text: string;
  truncated: boolean;
}

/** One sample with its headers, bodies and attempts (all already redacted). */
export interface SampleDetail {
  id: number;
  atMs: number;
  instance: string;
  requestId: string;
  triggers: SampleTrigger[];
  primary: SampleTrigger;
  subsystem: string;
  route: string;
  client: string;
  request: {
    method: string;
    path: string;
    query: string | null;
    headers: [string, string][];
    bytes: number;
    body: SampleBody | null;
    unusual?: string[];
  };
  response: {
    status: number;
    headers: [string, string][];
    bytes: number;
    body: SampleBody | null;
    bodyOmitted: string | null;
  };
  totalMs: number;
  upstream: { attempts: { endpoint: string; failure: string | null; status: number | null }[] };
  /** How many candidates of the same kind were dropped before this one. */
  suppressed?: { count: number; sinceMs: number };
}

export interface SampleFilters {
  trigger?: SampleTrigger;
  statusClass?: number;
  subsystem?: string;
  route?: string;
  instance?: string;
  client?: string;
  minMs?: number;
  sinceMs?: number;
}
