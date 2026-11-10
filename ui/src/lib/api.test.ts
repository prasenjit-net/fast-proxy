import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "./api";

function mockFetchOnce(response: Partial<Response> & { json?: () => Promise<unknown> }) {
  const fetchMock = vi.fn().mockResolvedValue(response as Response);
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("api client", () => {
  it("returns parsed JSON on a successful response", async () => {
    mockFetchOnce({
      ok: true,
      status: 200,
      json: () => Promise.resolve({ status: "ok", version: "0.1.0" }),
    });
    const result = await api.health();
    expect(result).toEqual({ status: "ok", version: "0.1.0" });
  });

  it("returns undefined for a 204 No Content response", async () => {
    mockFetchOnce({ ok: true, status: 204, json: () => Promise.reject("no body") });
    const result = await api.health();
    expect(result).toBeUndefined();
  });

  it("throws ApiError built from the backend's error envelope", async () => {
    mockFetchOnce({
      ok: false,
      status: 400,
      statusText: "Bad Request",
      json: () =>
        Promise.resolve({
          error: { code: "BAD_REQUEST", message: "payload failed validation" },
        }),
    });

    await expect(api.config()).rejects.toMatchObject({
      name: "ApiError",
      code: "BAD_REQUEST",
      status: 400,
      message: "payload failed validation",
    });
  });

  it("falls back to the HTTP status when the error body isn't JSON", async () => {
    mockFetchOnce({
      ok: false,
      status: 500,
      statusText: "Internal Server Error",
      json: () => Promise.reject(new Error("not json")),
    });

    await expect(api.health()).rejects.toMatchObject({
      code: "HTTP_500",
      status: 500,
      message: "Internal Server Error",
    });
  });

  it("wraps a network failure as a NETWORK ApiError", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new TypeError("Failed to fetch")),
    );

    await expect(api.health()).rejects.toBeInstanceOf(ApiError);
    await expect(api.health()).rejects.toMatchObject({ code: "NETWORK", status: 0 });
  });
});

describe("ApiError fields", () => {
  it("carries the server's per-field messages", async () => {
    mockFetchOnce({
      ok: false,
      status: 400,
      statusText: "Bad Request",
      json: () =>
        Promise.resolve({
          error: {
            code: "VALIDATION_FAILED",
            message: "validation failed",
            fields: [
              { field: "prefix", message: "must start with '/'" },
              { field: "connectTimeoutMs", message: "out of range" },
            ],
          },
        }),
    });

    const error = await api.config().catch((err: unknown) => err);
    expect(error).toBeInstanceOf(ApiError);
    const apiError = error as ApiError;
    expect(apiError.fields).toHaveLength(2);
    expect(apiError.fieldMessage("prefix")).toBe("must start with '/'");
    expect(apiError.fieldMessage("connectTimeoutMs")).toBe("out of range");
    expect(apiError.fieldMessage("name")).toBeUndefined();
  });

  it("has no fields for errors that do not name any", async () => {
    mockFetchOnce({
      ok: false,
      status: 409,
      statusText: "Conflict",
      json: () => Promise.resolve({ error: { code: "CONFLICT", message: "already exists" } }),
    });
    const error = (await api.config().catch((err: unknown) => err)) as ApiError;
    expect(error.fields).toEqual([]);
    expect(error.code).toBe("CONFLICT");
  });
});

describe("endpoint calls", () => {
  const route = {
    prefix: "/api",
    upstreamGroupId: 3,
    stripPrefix: true,
    upstreamPrefix: "/v2",
    preserveHost: false,
    connectTimeoutMs: 1000,
    responseTimeoutMs: 2000,
    retry: { maxAttempts: 2, idempotentOn5xx: true, onTimeout: false },
    enabled: true,
  };
  const settings = {
    name: "backend",
    lb: { algorithm: "round_robin" as const, hashKey: "client_ip" },
    failurePolicy: "fail_open" as const,
    health: {
      check: "none" as const,
      path: "/",
      expectedStatus: [200, 399] as [number, number],
      intervalMs: 5000,
      timeoutMs: 2000,
      rise: 2,
      fall: 3,
    },
    passive: { consecutiveFailures: 3, baseEjectionMs: 10000, maxEjectionMs: 300000, maxEjectionPercent: 50 },
    retryBudget: { percent: 20, minPerSec: 10 },
    tls: { enabled: false, verify: true, serverName: "", ca: "" },
  };
  const created = { ...settings, endpoints: [{ addr: "127.0.0.1:3000", weight: 1 }] };
  const endpoint = { addr: "127.0.0.1:3001", weight: 2 };
  const subsystem = { name: "shop", enabled: true, description: "" };
  const upload = { name: "shop", certPem: "cert", keyPem: "key" };

  const calls: [string, () => Promise<unknown>, string, string, unknown][] = [
    ["subsystems", () => api.subsystems(), "GET", "/api/subsystems", undefined],
    ["subsystem", () => api.subsystem(4), "GET", "/api/subsystems/4", undefined],
    ["createSubsystem", () => api.createSubsystem(subsystem), "POST", "/api/subsystems", subsystem],
    ["updateSubsystem", () => api.updateSubsystem(4, subsystem), "PUT", "/api/subsystems/4", subsystem],
    ["deleteSubsystem", () => api.deleteSubsystem(4), "DELETE", "/api/subsystems/4", undefined],
    ["addHost", () => api.addHost(4, "a.example.com"), "POST", "/api/subsystems/4/hosts", { pattern: "a.example.com" }],
    ["removeHost", () => api.removeHost(4, 9), "DELETE", "/api/subsystems/4/hosts/9", undefined],
    ["createUpstream", () => api.createUpstream(4, created), "POST", "/api/subsystems/4/upstreams", created],
    ["updateUpstream", () => api.updateUpstream(4, 7, settings), "PUT", "/api/subsystems/4/upstreams/7", settings],
    ["deleteUpstream", () => api.deleteUpstream(4, 7), "DELETE", "/api/subsystems/4/upstreams/7", undefined],
    ["upstreamStatus", () => api.upstreamStatus(4, 7), "GET", "/api/subsystems/4/upstreams/7/status", undefined],
    ["addEndpoint", () => api.addEndpoint(4, 7, endpoint), "POST", "/api/subsystems/4/upstreams/7/endpoints", endpoint],
    ["updateEndpoint", () => api.updateEndpoint(4, 7, 3, endpoint), "PUT", "/api/subsystems/4/upstreams/7/endpoints/3", endpoint],
    [
      "setEndpointState",
      () => api.setEndpointState(4, 7, 3, "draining"),
      "PUT",
      "/api/subsystems/4/upstreams/7/endpoints/3/state",
      { state: "draining" },
    ],
    ["removeEndpoint", () => api.removeEndpoint(4, 7, 3), "DELETE", "/api/subsystems/4/upstreams/7/endpoints/3", undefined],
    ["createRoute", () => api.createRoute(4, route), "POST", "/api/subsystems/4/routes", route],
    ["updateRoute", () => api.updateRoute(4, 5, route), "PUT", "/api/subsystems/4/routes/5", route],
    ["deleteRoute", () => api.deleteRoute(4, 5), "DELETE", "/api/subsystems/4/routes/5", undefined],
    ["revisions", () => api.revisions(), "GET", "/api/revisions?limit=25", undefined],
    ["revisions (older page)", () => api.revisions(40), "GET", "/api/revisions?limit=25&before=40", undefined],
    ["revision", () => api.revision(12), "GET", "/api/revisions/12", undefined],
    ["rollback", () => api.rollback(12), "POST", "/api/revisions/12/rollback", {}],
    ["resolve", () => api.resolve("a.com", "/x?y=1"), "POST", "/api/resolve", { host: "a.com", path: "/x?y=1" }],
    ["certificates", () => api.certificates(), "GET", "/api/certificates", undefined],
    ["uploadCertificate", () => api.uploadCertificate(upload), "POST", "/api/certificates", upload],
    [
      "generateSelfSigned",
      () => api.generateSelfSigned("dev", ["a.example.com"]),
      "POST",
      "/api/certificates/self-signed",
      { name: "dev", names: ["a.example.com"] },
    ],
    ["changeCertificate", () => api.changeCertificate(3, { name: "x" }), "PUT", "/api/certificates/3", { name: "x" }],
    ["deleteCertificate", () => api.deleteCertificate(3), "DELETE", "/api/certificates/3", undefined],
    ["uploadSubsystemCertificate", () => api.uploadSubsystemCertificate(4, upload), "POST", "/api/subsystems/4/certificate", upload],
    ["detachSubsystemCertificate", () => api.detachSubsystemCertificate(4), "DELETE", "/api/subsystems/4/certificate", undefined],
    ["caBundles", () => api.caBundles(), "GET", "/api/ca-bundles", undefined],
    ["uploadCaBundle", () => api.uploadCaBundle("corp", "pem"), "POST", "/api/ca-bundles", { name: "corp", pem: "pem" }],
    ["deleteCaBundle", () => api.deleteCaBundle(2), "DELETE", "/api/ca-bundles/2", undefined],
    ["tls", () => api.tls(), "GET", "/api/tls", undefined],
    ["status", () => api.status(), "GET", "/api/status", undefined],
    ["traffic", () => api.traffic(), "GET", "/api/traffic", undefined],
  ];

  it.each(calls)("%s", async (_name, call, method, path, body) => {
    const fetchMock = mockFetchOnce({ ok: true, status: 200, json: () => Promise.resolve({}) });
    await call();
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe(path);
    expect(init.method ?? "GET").toBe(method);
    if (body === undefined) {
      expect(init.body).toBeUndefined();
    } else {
      expect(JSON.parse(init.body as string)).toEqual(body);
    }
  });
});
