// Typed API client: one function per endpoint. Failures surface as ApiError
// (see http.ts); types live in types.ts and are re-exported from here.
import { request, send } from "./http";
import type {
  Envelope,
  Host,
  Resolution,
  RevisionDetail,
  RevisionPage,
  Route,
  RouteInput,
  ServerConfig,
  Status,
  SubsystemDetail,
  SubsystemInput,
  SubsystemSummary,
  Traffic,
  Upstream,
  UpstreamInput,
  VersionOnly,
} from "./types";

export { ApiError, type FieldError } from "./http";
export * from "./types";

const subsystem = (id: number) => `/api/subsystems/${id}`;

export const api = {
  config: () => request<ServerConfig>("/api/config"),
  health: () => request<{ status: string; version: string }>("/api/health"),
  status: () => request<Envelope<Status>>("/api/status"),
  traffic: () => request<Traffic>("/api/traffic"),

  // subsystems
  subsystems: () => request<Envelope<SubsystemSummary[]>>("/api/subsystems"),
  subsystem: (id: number) => request<Envelope<SubsystemDetail>>(subsystem(id)),
  createSubsystem: (input: SubsystemInput) =>
    request<Envelope<SubsystemDetail>>("/api/subsystems", send("POST", input)),
  updateSubsystem: (id: number, input: SubsystemInput) =>
    request<Envelope<SubsystemDetail>>(subsystem(id), send("PUT", input)),
  deleteSubsystem: (id: number) => request<VersionOnly>(subsystem(id), send("DELETE")),

  // hosts
  addHost: (id: number, pattern: string) =>
    request<Envelope<Host>>(`${subsystem(id)}/hosts`, send("POST", { pattern })),
  removeHost: (id: number, hostId: number) =>
    request<VersionOnly>(`${subsystem(id)}/hosts/${hostId}`, send("DELETE")),

  // upstreams
  createUpstream: (id: number, input: UpstreamInput) =>
    request<Envelope<Upstream>>(`${subsystem(id)}/upstreams`, send("POST", input)),
  updateUpstream: (id: number, upstreamId: number, input: UpstreamInput) =>
    request<Envelope<Upstream>>(`${subsystem(id)}/upstreams/${upstreamId}`, send("PUT", input)),
  deleteUpstream: (id: number, upstreamId: number) =>
    request<VersionOnly>(`${subsystem(id)}/upstreams/${upstreamId}`, send("DELETE")),

  // routes
  createRoute: (id: number, input: RouteInput) =>
    request<Envelope<Route>>(`${subsystem(id)}/routes`, send("POST", input)),
  updateRoute: (id: number, routeId: number, input: RouteInput) =>
    request<Envelope<Route>>(`${subsystem(id)}/routes/${routeId}`, send("PUT", input)),
  deleteRoute: (id: number, routeId: number) =>
    request<VersionOnly>(`${subsystem(id)}/routes/${routeId}`, send("DELETE")),

  // revisions
  revisions: (before?: number | null) =>
    request<RevisionPage>(`/api/revisions?limit=25${before ? `&before=${before}` : ""}`),
  revision: (version: number) =>
    request<Envelope<RevisionDetail>>(`/api/revisions/${version}`),
  rollback: (version: number) =>
    request<Envelope<{ restoredFrom: number }>>(
      `/api/revisions/${version}/rollback`,
      send("POST", {}),
    ),

  // resolver
  resolve: (host: string, path: string) =>
    request<Envelope<Resolution>>("/api/resolve", send("POST", { host, path })),
};
