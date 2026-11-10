import type {
  Endpoint,
  EndpointStatus,
  Envelope,
  RevisionPage,
  Route,
  SubsystemDetail,
  SubsystemSummary,
  Upstream,
  UpstreamStatus,
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

/** A subsystem with one host, one upstream, and two routes (in match order). */
export const subsystemDetail = (overrides: Partial<SubsystemDetail> = {}): SubsystemDetail => ({
  id: 1,
  name: "shop",
  enabled: true,
  description: "The shop",
  createdAtMs: NOW,
  updatedAtMs: NOW,
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
