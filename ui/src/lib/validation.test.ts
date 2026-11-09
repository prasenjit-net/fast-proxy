import { describe, expect, it } from "vitest";
import {
  validateHost,
  validateRoute,
  validateSubsystem,
  validateUpstream,
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

describe("validateUpstream", () => {
  const ok = { name: "backend", addr: "127.0.0.1:3000", weight: "1" };

  it("accepts hostnames, IPs and bracketed IPv6 with a port", () => {
    for (const addr of ["backend:8080", "127.0.0.1:3000", "[::1]:3000", "a.internal:65535"]) {
      expect(validateUpstream({ ...ok, addr }).value?.addr).toBe(addr);
    }
  });

  it("builds numeric fields", () => {
    expect(validateUpstream({ ...ok, weight: " 3 " }).value).toEqual({
      name: "backend",
      addr: "127.0.0.1:3000",
      weight: 3,
    });
  });

  it.each(["", "backend", "backend:", ":80", "backend:0", "backend:65536", "backend:http"])(
    "rejects the address %j",
    (addr) => {
      expect(validateUpstream({ ...ok, addr }).errors.addr).toBeDefined();
    },
  );

  it("rejects an empty or overlong name and a bad weight", () => {
    expect(validateUpstream({ ...ok, name: " " }).errors.name).toBeDefined();
    expect(validateUpstream({ ...ok, name: "x".repeat(64) }).errors.name).toBeDefined();
    for (const weight of ["0", "-1", "1.5", "", "abc"]) {
      expect(validateUpstream({ ...ok, weight }).errors.weight).toBeDefined();
    }
  });

  it("reports every problem at once", () => {
    const result = validateUpstream({ name: "", addr: "nope", weight: "0" });
    expect(Object.keys(result.errors)).toEqual(["name", "addr", "weight"]);
  });
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
