// Typed API client: one function per endpoint. Failures surface as ApiError
// (see http.ts); types live in types.ts and are re-exported from here.
import { request, send } from "./http";
import type {
  CaBundle,
  Certificate,
  CertificateChange,
  CertificateUpload,
  Envelope,
  Host,
  Resolution,
  RevisionDetail,
  RevisionPage,
  Route,
  RouteInput,
  ServerConfig,
  EndpointInput,
  EndpointState,
  Endpoint,
  Status,
  TlsStatus,
  SubsystemDetail,
  SubsystemInput,
  SubsystemSummary,
  Traffic,
  Upstream,
  UpstreamCreate,
  UpstreamSettings,
  UpstreamStatus,
  VersionOnly,
  AuditFilters,
  AuditPage,
  CreatedToken,
  Token,
  TokenInput,
  WhoAmI,
} from "./types";

export { ApiError, type FieldError } from "./http";
export * from "./types";

const subsystem = (id: number) => `/api/subsystems/${id}`;

function auditQuery(filters: AuditFilters, before: number | null): string {
  const params = new URLSearchParams({ limit: "50" });
  if (before) params.set("before", String(before));
  if (filters.actor) params.set("actor", filters.actor);
  if (filters.action) params.set("action", filters.action);
  if (filters.outcome) params.set("outcome", filters.outcome);
  if (filters.sinceMs) params.set("since", String(filters.sinceMs));
  return params.toString();
}

export const api = {
  /** Who the token (this tab's, or `token` when given) belongs to; a given token is not stored. */
  whoami: (token?: string) =>
    request<Envelope<WhoAmI>>(
      "/api/whoami",
      token ? { headers: { Authorization: `Bearer ${token}` } } : undefined,
    ),
  /** A single-use ticket (30 s) to open the WebSocket, which cannot carry a header. */
  wsTicket: () => request<{ ticket: string; expiresInSecs: number }>("/api/ws-ticket", send("POST", {})),

  // tokens (admin only)
  tokens: () => request<Envelope<Token[]>>("/api/tokens"),
  createToken: (input: TokenInput) =>
    request<Envelope<CreatedToken>>("/api/tokens", send("POST", input)),
  revokeToken: (id: number) => request<VersionOnly>(`/api/tokens/${id}`, send("DELETE")),

  // the audit log
  audit: (filters: AuditFilters = {}, before: number | null = null) =>
    request<AuditPage>(`/api/audit?${auditQuery(filters, before)}`),
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

  // upstreams and their endpoints
  createUpstream: (id: number, input: UpstreamCreate) =>
    request<Envelope<Upstream>>(`${subsystem(id)}/upstreams`, send("POST", input)),
  updateUpstream: (id: number, upstreamId: number, input: UpstreamSettings) =>
    request<Envelope<Upstream>>(`${subsystem(id)}/upstreams/${upstreamId}`, send("PUT", input)),
  deleteUpstream: (id: number, upstreamId: number) =>
    request<VersionOnly>(`${subsystem(id)}/upstreams/${upstreamId}`, send("DELETE")),
  upstreamStatus: (id: number, upstreamId: number) =>
    request<Envelope<UpstreamStatus>>(`${subsystem(id)}/upstreams/${upstreamId}/status`),
  addEndpoint: (id: number, upstreamId: number, input: EndpointInput) =>
    request<Envelope<Endpoint>>(`${subsystem(id)}/upstreams/${upstreamId}/endpoints`, send("POST", input)),
  updateEndpoint: (id: number, upstreamId: number, endpointId: number, input: EndpointInput) =>
    request<Envelope<Endpoint>>(
      `${subsystem(id)}/upstreams/${upstreamId}/endpoints/${endpointId}`,
      send("PUT", input),
    ),
  setEndpointState: (id: number, upstreamId: number, endpointId: number, state: EndpointState) =>
    request<Envelope<Endpoint>>(
      `${subsystem(id)}/upstreams/${upstreamId}/endpoints/${endpointId}/state`,
      send("PUT", { state }),
    ),
  removeEndpoint: (id: number, upstreamId: number, endpointId: number) =>
    request<VersionOnly>(`${subsystem(id)}/upstreams/${upstreamId}/endpoints/${endpointId}`, send("DELETE")),

  // routes
  createRoute: (id: number, input: RouteInput) =>
    request<Envelope<Route>>(`${subsystem(id)}/routes`, send("POST", input)),
  updateRoute: (id: number, routeId: number, input: RouteInput) =>
    request<Envelope<Route>>(`${subsystem(id)}/routes/${routeId}`, send("PUT", input)),
  deleteRoute: (id: number, routeId: number) =>
    request<VersionOnly>(`${subsystem(id)}/routes/${routeId}`, send("DELETE")),

  // certificates (private keys are write-only: no response carries one)
  certificates: () => request<Envelope<Certificate[]>>("/api/certificates"),
  uploadCertificate: (input: CertificateUpload) =>
    request<Envelope<Certificate>>("/api/certificates", send("POST", input)),
  generateSelfSigned: (name: string, names: string[]) =>
    request<Envelope<Certificate>>("/api/certificates/self-signed", send("POST", { name, names })),
  changeCertificate: (id: number, change: CertificateChange) =>
    request<Envelope<Certificate>>(`/api/certificates/${id}`, send("PUT", change)),
  deleteCertificate: (id: number) => request<VersionOnly>(`/api/certificates/${id}`, send("DELETE")),
  uploadSubsystemCertificate: (id: number, input: CertificateUpload) =>
    request<Envelope<Certificate>>(`${subsystem(id)}/certificate`, send("POST", input)),
  detachSubsystemCertificate: (id: number) =>
    request<VersionOnly>(`${subsystem(id)}/certificate`, send("DELETE")),

  // CA bundles and the process TLS state
  caBundles: () => request<Envelope<CaBundle[]>>("/api/ca-bundles"),
  uploadCaBundle: (name: string, pem: string) =>
    request<Envelope<CaBundle>>("/api/ca-bundles", send("POST", { name, pem })),
  deleteCaBundle: (id: number) => request<VersionOnly>(`/api/ca-bundles/${id}`, send("DELETE")),
  tls: () => request<Envelope<TlsStatus>>("/api/tls"),

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
