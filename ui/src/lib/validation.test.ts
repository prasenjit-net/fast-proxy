import { describe, expect, it } from "vitest";
import {
  validateHost,
  validateRoute,
  validateSubsystem,
  type RouteValues,
} from "./validation";

describe("validateSubsystem", () => {
  const ok = { name: "my-shop", description: "", enabled: true };

  it("accepts a valid name and builds the request body", () => {
    expect(validateSubsystem({ ...ok, name: "  my-shop  " }).value).toEqual(ok);
  });

  it.each(["", "Shop", "-a", "a-", "my_shop", "my shop", "a".repeat(64)])(
    "rejects the name %j",
    (name) => {
      const result = validateSubsystem({ ...ok, name });
      expect(result.errors.name).toBeDefined();
      expect(result.value).toBeUndefined();
    },
  );

  it("limits the description to 500 characters", () => {
    expect(validateSubsystem({ ...ok, description: "x".repeat(500) }).value).toBeDefined();
    expect(validateSubsystem({ ...ok, description: "x".repeat(501) }).errors.description).toBeDefined();
  });

  it("reports every problem at once", () => {
    const result = validateSubsystem({ name: "Bad Name", description: "x".repeat(501), enabled: true });
    expect(Object.keys(result.errors)).toEqual(["name", "description"]);
  });
});

describe("validateHost", () => {
  it("accepts exact and wildcard patterns", () => {
    expect(validateHost("shop.example.com").value).toBe("shop.example.com");
    expect(validateHost(" *.example.com ").value).toBe("*.example.com");
  });

  it.each(["", "  ", "shop.example.com:8080", "http://shop.example.com", "a b.com", "a.com/x"])(
    "rejects %j",
    (pattern) => {
      expect(validateHost(pattern).errors.pattern).toBeDefined();
    },
  );
});

describe("validateRoute", () => {
  const ok: RouteValues = {
    prefix: "/api/",
    upstreamGroupId: "3",
    stripPrefix: true,
    upstreamPrefix: "",
    preserveHost: true,
    connectTimeoutMs: "5000",
    responseTimeoutMs: "30000",
    retryMaxAttempts: "2",
    retryIdempotentOn5xx: true,
    retryOnTimeout: false,
    enabled: true,
  };

  it("builds the request body", () => {
    expect(validateRoute(ok).value).toEqual({
      prefix: "/api/",
      upstreamGroupId: 3,
      stripPrefix: true,
      upstreamPrefix: "",
      preserveHost: true,
      connectTimeoutMs: 5000,
      responseTimeoutMs: 30000,
      retry: { maxAttempts: 2, idempotentOn5xx: true, onTimeout: false },
      enabled: true,
    });
  });

  it.each(["", "api", "/a?x=1", "/a#f", "/a b", "/a//b", "/a/../b", "/./a", "/a\\b"])(
    "rejects the prefix %j",
    (prefix) => {
      expect(validateRoute({ ...ok, prefix }).errors.prefix).toBeDefined();
    },
  );

  it("accepts an empty upstream prefix but checks a given one", () => {
    expect(validateRoute({ ...ok, upstreamPrefix: "" }).value).toBeDefined();
    expect(validateRoute({ ...ok, upstreamPrefix: "/v2" }).value?.upstreamPrefix).toBe("/v2");
    expect(validateRoute({ ...ok, upstreamPrefix: "v2" }).errors.upstreamPrefix).toBeDefined();
  });

  it("requires an upstream", () => {
    expect(validateRoute({ ...ok, upstreamGroupId: "" }).errors.upstreamGroupId).toBeDefined();
  });

  it("bounds the timeouts to 100 ms through 10 minutes", () => {
    for (const value of ["100", "600000"]) {
      expect(validateRoute({ ...ok, connectTimeoutMs: value }).value).toBeDefined();
    }
    for (const value of ["99", "600001", "", "1.5", "x"]) {
      const result = validateRoute({ ...ok, connectTimeoutMs: value, responseTimeoutMs: value });
      expect(result.errors.connectTimeoutMs).toBeDefined();
      expect(result.errors.responseTimeoutMs).toBeDefined();
    }
  });

  it.each(["0", "6", "", "x", "1.5"])("rejects %j retry attempts", (retryMaxAttempts) => {
    expect(validateRoute({ ...ok, retryMaxAttempts }).errors["retry.maxAttempts"]).toBeDefined();
  });

  it("builds the retry settings from the toggles and attempts", () => {
    const result = validateRoute({
      ...ok,
      retryMaxAttempts: "5",
      retryIdempotentOn5xx: false,
      retryOnTimeout: true,
    });
    expect(result.value?.retry).toEqual({ maxAttempts: 5, idempotentOn5xx: false, onTimeout: true });
    expect(validateRoute({ ...ok, retryMaxAttempts: "1" }).value?.retry.maxAttempts).toBe(1);
  });

  it("reports every problem at once, in field order", () => {
    const result = validateRoute({
      ...ok,
      prefix: "no-slash",
      upstreamPrefix: "bad",
      upstreamGroupId: "",
      connectTimeoutMs: "5",
      responseTimeoutMs: "5",
    });
    expect(Object.keys(result.errors)).toEqual([
      "prefix",
      "upstreamPrefix",
      "upstreamGroupId",
      "connectTimeoutMs",
      "responseTimeoutMs",
    ]);
  });
});
