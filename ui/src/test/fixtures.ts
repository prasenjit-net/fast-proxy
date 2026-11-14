import type {
  AdminStatus,
  AuditEntry,
  CaBundle,
  Certificate,
  Endpoint,
  EndpointStatus,
  Envelope,
  HttpsSettings,
  MaintenanceScope,
  RevisionPage,
  Route,
  SampleDetail,
  SampleSummary,
  SamplingOverride,
  SamplingSettings,
  SamplingView,
  SubsystemDetail,
  StatusTls,
  SubsystemSummary,
  TlsStatus,
  Token,
  Upstream,
  UpstreamStatus,
  WhoAmI,
} from "../lib/types";

export const NOW = 1_700_000_000_000;

export const endpoint = (overrides: Partial<Endpoint> = {}): Endpoint => ({
  id: 11,
  addr: "127.0.0.1:3000",
  weight: 1,
  state: "enabled",
  ...overrides,
});

/** An upstream with the server's default policy and one endpoint. */
export const upstream = (overrides: Partial<Upstream> = {}): Upstream => ({
  id: 5,
  name: "backend",
  lb: { algorithm: "round_robin", hashKey: "client_ip" },
  failurePolicy: "fail_open",
  health: {
    check: "none",
    path: "/",
    expectedStatus: [200, 399],
    intervalMs: 5000,
    timeoutMs: 2000,
    rise: 2,
    fall: 3,
  },
  passive: {
    consecutiveFailures: 3,
    baseEjectionMs: 10000,
    maxEjectionMs: 300000,
    maxEjectionPercent: 50,
  },
  retryBudget: { percent: 20, minPerSec: 10 },
  tls: { enabled: false, verify: true, serverName: "", ca: "" },
  endpoints: [endpoint()],
  ...overrides,
});

export const endpointStatus = (overrides: Partial<EndpointStatus> = {}): EndpointStatus => ({
  id: 11,
  addr: "127.0.0.1:3000",
  weight: 1,
  state: "enabled",
  effective: "healthy",
  reason: "taking requests",
  inFlight: 0,
  ewmaMs: null,
  lastProbe: null,
  ...overrides,
});

export const upstreamStatus = (
  endpoints: EndpointStatus[] = [endpointStatus()],
  overrides: Partial<UpstreamStatus> = {},
): UpstreamStatus => ({
  name: "backend",
  algorithm: "round_robin",
  failurePolicy: "fail_open",
  healthy: endpoints.filter((e) => e.effective === "healthy").length,
  endpoints,
  ...overrides,
});

export const route = (overrides: Partial<Route> = {}): Route => ({
  id: 6,
  prefix: "/",
  upstreamGroupId: 5,
  stripPrefix: false,
  upstreamPrefix: "",
  preserveHost: true,
  connectTimeoutMs: 5000,
  responseTimeoutMs: 30000,
  retry: { maxAttempts: 2, idempotentOn5xx: true, onTimeout: false },
  enabled: true,
  ...overrides,
});

/** HTTPS off, with the server's defaults. */
export function httpsSettings(overrides: Partial<HttpsSettings> = {}): HttpsSettings {
  return {
    enabled: false,
    certificateId: null,
    redirect: "off",
    hsts: { enabled: false, maxAgeSecs: 31_536_000, includeSubdomains: false },
    effective: false,
    certificate: null,
    ...overrides,
  };
}

export const certificate = (overrides: Partial<Certificate> = {}): Certificate => ({
  id: 3,
  name: "shop",
  source: "uploaded",
  names: ["shop.example.com"],
  subject: "CN=shop.example.com",
  issuer: "CN=Test CA",
  notBeforeMs: NOW - 86_400_000,
  notAfterMs: NOW + 90 * 86_400_000,
  secondsLeft: 90 * 86_400,
  state: "ok",
  fingerprint: "ab".repeat(32),
  usedBy: [],
  updatedAtMs: NOW,
  ...overrides,
});

export const caBundle = (overrides: Partial<CaBundle> = {}): CaBundle => ({
  id: 2,
  name: "corp",
  certCount: 1,
  certificates: [{ subject: "CN=Corp Root", notAfterMs: NOW + 365 * 86_400_000 }],
  updatedAtMs: NOW,
  ...overrides,
});

export const tlsStatus = (overrides: Partial<TlsStatus> = {}): TlsStatus => ({
  enabled: true,
  listen: "0.0.0.0:8443",
  http2: true,
  minVersion: "1.2",
  vault: "ok",
  ...overrides,
});

export const statusTls = (overrides: Partial<StatusTls> = {}): StatusTls => ({
  enabled: true,
  listen: "0.0.0.0:8443",
  vault: "ok",
  certificates: 0,
  unreadable: 0,
  soonestExpiry: null,
  ...overrides,
});

/** A subsystem with one host, one upstream, and two routes (in match order). */
export const subsystemDetail = (overrides: Partial<SubsystemDetail> = {}): SubsystemDetail => ({
  id: 1,
  name: "shop",
  enabled: true,
  description: "The shop",
  createdAtMs: NOW,
  updatedAtMs: NOW,
  https: httpsSettings(),
  hosts: [{ id: 10, pattern: "shop.example.com" }],
  upstreams: [upstream()],
  routes: [
    route({
      id: 7,
      prefix: "/api",
      stripPrefix: true,
      upstreamPrefix: "/v2",
      preserveHost: false,
      connectTimeoutMs: 1000,
      responseTimeoutMs: 2000,
    }),
    route(),
  ],
  ...overrides,
});

export const subsystemSummary = (overrides: Partial<SubsystemSummary> = {}): SubsystemSummary => ({
  id: 1,
  name: "shop",
  enabled: true,
  description: "The shop",
  hosts: ["shop.example.com"],
  routeCount: 2,
  updatedAtMs: NOW,
  ...overrides,
});

export const envelope = <T>(data: T, version = 4): Envelope<T> => ({ data, version });

export const revisionPage = (overrides: Partial<RevisionPage> = {}): RevisionPage => ({
  data: [
    { version: 4, createdAtMs: NOW, summary: "route created: / in shop" },
    { version: 3, createdAtMs: NOW - 1000, summary: "upstream created: backend in shop" },
  ],
  version: 4,
  nextBefore: null,
  ...overrides,
});

// ---- sign-in, tokens and the audit log ----

export const adminStatus = (overrides: Partial<AdminStatus> = {}): AdminStatus => ({
  auth: "required",
  protectedChannel: true,
  tlsCertificate: null,
  activeTokens: 2,
  activeMaintenance: 0,
  ...overrides,
});

export const whoami = (overrides: Partial<WhoAmI> = {}): WhoAmI => ({
  name: "test-admin",
  role: "admin",
  tokenExpiresAtMs: null,
  protectedChannel: true,
  auth: "required",
  ...overrides,
});

export const token = (overrides: Partial<Token> = {}): Token => ({
  id: 1,
  name: "ci",
  role: "viewer",
  createdAtMs: NOW - 3_600_000,
  createdBy: "test-admin",
  expiresAtMs: null,
  revokedAtMs: null,
  lastUsedAtMs: null,
  ...overrides,
});

export const auditEntry = (overrides: Partial<AuditEntry> = {}): AuditEntry => ({
  id: 10,
  atMs: Date.now() - 60_000,
  instance: "i-1",
  actor: "test-admin",
  role: "admin",
  source: "127.0.0.1",
  requestId: "abc-1",
  action: "subsystem.create",
  target: "shop",
  outcome: "ok",
  detail: { summary: "subsystem created: shop" },
  revision: 3,
  ...overrides,
});

/** A scope in maintenance for the whole proxy, ending an hour after the real clock now. */
export const maintenanceScope = (overrides: Partial<MaintenanceScope> = {}): MaintenanceScope => ({
  scope: "global",
  subsystemId: null,
  subsystem: null,
  routeId: null,
  route: null,
  message: "Back at 14:00",
  contentType: null,
  hasCustomBody: false,
  retryAfterSecs: null,
  expiresAtMs: Date.now() + 3_600_000,
  secondsLeft: 3600,
  setBy: "test-admin",
  setAtMs: Date.now() - 60_000,
  bypass: { cidrs: [], header: null, secretSet: false },
  ...overrides,
});

export const samplingSettings = (overrides: Partial<SamplingSettings> = {}): SamplingSettings => ({
  enabled: true,
  triggers: ["upstream_failure", "slow"],
  slowThresholdMs: 2000,
  slowP99FactorX10: 30,
  slowP99FloorMs: 50,
  clientErrorsPerMin: 100,
  oversizedRequestBytes: 8 * 1024 * 1024,
  oversizedResponseBytes: 32 * 1024 * 1024,
  baselinePerMille: 1,
  ratePerSec: 20,
  rateBurst: 40,
  stormBurst: 3,
  stormIntervalSecs: 30,
  retentionDays: 7,
  maxRows: 100_000,
  maxBytes: 256 * 1024 * 1024,
  bodyRequestCap: 4096,
  bodyResponseCap: 4096,
  extraHeaders: [],
  redactHeaders: [],
  redactQueryParams: [],
  redactBodyFields: ["iban"],
  builtInRedaction: { headers: ["authorization"], queryParams: ["token"], bodyFields: ["password"] },
  updatedBy: "ann",
  updatedAtMs: NOW,
  ...overrides,
});

export const samplingOverride = (overrides: Partial<SamplingOverride> = {}): SamplingOverride => ({
  scope: "route",
  subsystemId: null,
  subsystem: "shop",
  routeId: 7,
  route: "/api",
  enabled: null,
  captureBodies: null,
  baselinePerMille: null,
  slowThresholdMs: null,
  slowP99FactorX10: null,
  setBy: "ann",
  setAtMs: NOW,
  ...overrides,
});

export const samplingView = (overrides: Partial<SamplingView> = {}): SamplingView => ({
  settings: samplingSettings(),
  overrides: [],
  status: {
    processEnabled: true,
    captured: 42,
    dropped: 3,
    table: { rows: 10, bytes: 2048, oldestAtMs: NOW - 1000 },
  },
  ...overrides,
});

export const sampleSummary = (overrides: Partial<SampleSummary> = {}): SampleSummary => ({
  id: 1,
  atMs: NOW,
  instance: "i-1",
  requestId: "req-1",
  triggers: ["upstream_failure", "slow"],
  primary: "upstream_failure",
  subsystem: "shop",
  route: "/api",
  client: "203.0.113.7",
  method: "GET",
  status: 502,
  totalMs: 1500,
  sizeBytes: 900,
  ...overrides,
});

export const sampleDetail = (overrides: Partial<SampleDetail> = {}): SampleDetail => ({
  id: 1,
  atMs: NOW,
  instance: "i-1",
  requestId: "req-1",
  triggers: ["upstream_failure"],
  primary: "upstream_failure",
  subsystem: "shop",
  route: "/api",
  client: "203.0.113.7",
  request: {
    method: "GET",
    path: "/api/orders",
    query: "token=%5Bredacted%5D",
    headers: [["user-agent", "curl"]],
    bytes: 0,
    body: null,
  },
  response: { status: 502, headers: [], bytes: 0, body: null, bodyOmitted: "bodies are off" },
  totalMs: 1500,
  upstream: { attempts: [{ endpoint: "10.0.0.1:80", failure: "connect_refused", status: null }] },
  ...overrides,
});
