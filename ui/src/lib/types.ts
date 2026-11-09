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

export interface Upstream {
  id: number;
  name: string;
  addr: string;
  weight: number;
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

export interface UpstreamInput {
  name: string;
  addr: string;
  weight: number;
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

export interface Status {
  listeners: { name: string; addr: string }[];
  database: string;
  subsystems: { total: number; enabled: number };
  startedAtMs: number;
  uptimeSecs: number;
}
