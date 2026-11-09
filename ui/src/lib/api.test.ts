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
    const result = await api.deleteTask(1);
    expect(result).toBeUndefined();
  });

  it("throws ApiError built from the backend's error envelope", async () => {
    mockFetchOnce({
      ok: false,
      status: 400,
      statusText: "Bad Request",
      json: () =>
        Promise.resolve({
          error: { code: "BAD_REQUEST", message: "task title must not be empty" },
        }),
    });

    await expect(api.createTask("")).rejects.toMatchObject({
      name: "ApiError",
      code: "BAD_REQUEST",
      status: 400,
      message: "task title must not be empty",
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
