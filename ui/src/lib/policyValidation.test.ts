import { describe, expect, it } from "vitest";
import {
  defaultPolicyValues,
  policyValuesFrom,
  validateEndpoint,
  validatePolicy,
  type PolicyValues,
} from "./policyValidation";
import { upstream } from "../test/fixtures";

const ok = defaultPolicyValues();
const with_ = (patch: Partial<PolicyValues>): PolicyValues => ({ ...ok, ...patch });

describe("validatePolicy", () => {
  it("turns the server's defaults into the server's default policy", () => {
    const { health, passive, retryBudget, lb, failurePolicy } = upstream();
    expect(validatePolicy(ok).value).toEqual({ lb, failurePolicy, health, passive, retryBudget });
  });

  it("round-trips an existing group's policy through the form values", () => {
    const group = upstream({
      lb: { algorithm: "consistent_hash", hashKey: "header:x-user" },
      failurePolicy: "fail_closed",
      health: {
        check: "http",
        path: "/healthz",
        expectedStatus: [200, 299],
        intervalMs: 2000,
        timeoutMs: 500,
        rise: 3,
        fall: 4,
      },
    });
    const values = policyValuesFrom(group);
    expect(values.hashKind).toBe("header");
    expect(values.hashName).toBe("x-user");
    const { value } = validatePolicy(values);
    expect(value?.lb).toEqual(group.lb);
    expect(value?.health).toEqual(group.health);
    expect(value?.failurePolicy).toBe("fail_closed");
  });

  it("only hashes on a header or cookie when the algorithm hashes", () => {
    const stale = with_({ algorithm: "round_robin", hashKind: "header", hashName: "!!" });
    expect(validatePolicy(stale).value?.lb.hashKey).toBe("client_ip");
    const hashing = with_({ algorithm: "consistent_hash", hashKind: "cookie", hashName: "sid" });
    expect(validatePolicy(hashing).value?.lb.hashKey).toBe("cookie:sid");
    const bad = with_({ algorithm: "consistent_hash", hashKind: "header", hashName: "a b" });
    expect(validatePolicy(bad).errors["lb.hashKey"]).toBeDefined();
  });

  it("ignores the probe details while health checks are off", () => {
    const result = validatePolicy(with_({ healthCheck: "none", intervalMs: "garbage", rise: "0" }));
    expect(result.errors).toEqual({});
    expect(result.value?.health.check).toBe("none");
  });

  it("checks every probe bound with the server's field names", () => {
    const http = with_({ healthCheck: "http" });
    const cases: [Partial<PolicyValues>, string][] = [
      [{ healthPath: "healthz" }, "health.path"],
      [{ healthPath: "/a b" }, "health.path"],
      [{ expectedMin: "99" }, "health.expectedStatus"],
      [{ expectedMax: "600" }, "health.expectedStatus"],
      [{ expectedMin: "300", expectedMax: "200" }, "health.expectedStatus"],
      [{ intervalMs: "99" }, "health.intervalMs"],
      [{ timeoutMs: "99" }, "health.timeoutMs"],
      [{ timeoutMs: "6000" }, "health.timeoutMs"],
      [{ rise: "0" }, "health.rise"],
      [{ fall: "11" }, "health.fall"],
    ];
    for (const [patch, field] of cases) {
      expect(validatePolicy({ ...http, ...patch }).errors[field], field).toBeDefined();
    }
  });

  it("does not need a path for a TCP check", () => {
    expect(validatePolicy(with_({ healthCheck: "tcp", healthPath: "nonsense" })).errors).toEqual({});
  });

  it("checks the ejection and retry budget bounds", () => {
    const cases: [Partial<PolicyValues>, string][] = [
      [{ passiveFailures: "101" }, "passive.consecutiveFailures"],
      [{ baseEjectionMs: "99" }, "passive.baseEjectionMs"],
      [{ maxEjectionMs: "5000" }, "passive.maxEjectionMs"],
      [{ maxEjectionPercent: "0" }, "passive.maxEjectionPercent"],
      [{ maxEjectionPercent: "101" }, "passive.maxEjectionPercent"],
      [{ retryPercent: "101" }, "retryBudget.percent"],
      [{ retryMinPerSec: "1001" }, "retryBudget.minPerSec"],
    ];
    for (const [patch, field] of cases) {
      expect(validatePolicy(with_(patch)).errors[field], field).toBeDefined();
    }
    expect(validatePolicy(with_({ passiveFailures: "0", retryPercent: "0", retryMinPerSec: "0" })).errors).toEqual({});
  });

  it("reports problems from every section at once", () => {
    const result = validatePolicy(
      with_({ healthCheck: "tcp", rise: "0", passiveFailures: "x", retryPercent: "200" }),
    );
    expect(Object.keys(result.errors)).toEqual(
      expect.arrayContaining(["health.rise", "passive.consecutiveFailures", "retryBudget.percent"]),
    );
    expect(result.value).toBeUndefined();
  });
});

describe("validateEndpoint", () => {
  it("accepts hostnames, IPs and bracketed IPv6 with a port", () => {
    for (const addr of ["backend:8080", "127.0.0.1:3000", "[::1]:3000", "a.internal:65535"]) {
      expect(validateEndpoint({ addr, weight: "1" }).value?.addr).toBe(addr);
    }
  });

  it("builds the weight as a number and trims the address", () => {
    expect(validateEndpoint({ addr: " a:1 ", weight: " 1000 " }).value).toEqual({ addr: "a:1", weight: 1000 });
  });

  it.each(["", "backend", "backend:", ":80", "backend:0", "backend:65536", "backend:http"])(
    "rejects the address %j",
    (addr) => {
      expect(validateEndpoint({ addr, weight: "1" }).errors.addr).toBeDefined();
    },
  );

  it.each(["0", "1001", "-1", "1.5", "", "abc"])("rejects the weight %j", (weight) => {
    expect(validateEndpoint({ addr: "a:1", weight }).errors.weight).toBeDefined();
  });

  it("namespaces errors when asked, to match the server's nested names", () => {
    const result = validateEndpoint({ addr: "nope", weight: "0" }, "endpoints.0.");
    expect(Object.keys(result.errors)).toEqual(["endpoints.0.addr", "endpoints.0.weight"]);
  });
});
