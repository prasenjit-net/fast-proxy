import { describe, expect, it } from "vitest";
import {
  defaultRateLimitValues,
  rateLimitSummary,
  rateLimitValuesFrom,
  validateRateLimit,
  type RateLimitValues,
} from "./rateLimitValidation";
import type { RateLimit } from "./types";

const ok = defaultRateLimitValues();
const with_ = (patch: Partial<RateLimitValues>): RateLimitValues => ({ ...ok, ...patch });
const policy = (patch: Partial<RateLimit> = {}): RateLimit => ({
  rate: 10,
  periodSecs: 60,
  burst: 10,
  key: "client",
  header: null,
  ipv6Prefix: 64,
  ...patch,
});

describe("validateRateLimit", () => {
  it("sends only the rate and period when the rest is the server's default", () => {
    expect(validateRateLimit(ok).value).toEqual({ rate: 10, periodSecs: 1 });
  });

  it("turns the period unit into seconds", () => {
    expect(validateRateLimit(with_({ periodAmount: "5", periodUnit: "m" })).value?.periodSecs).toBe(300);
    expect(validateRateLimit(with_({ periodAmount: "1", periodUnit: "h" })).value?.periodSecs).toBe(3600);
    for (const [periodAmount, periodUnit] of [["0", "s"], ["2", "h"], ["61", "m"], ["x", "s"]] as const) {
      expect(validateRateLimit(with_({ periodAmount, periodUnit })).errors["rateLimit.periodSecs"]).toBeDefined();
    }
  });

  it("bounds the rate and the burst, and asks for a burst when the rate is above its cap", () => {
    for (const rate of ["0", "1000001", "", "1.5"]) {
      expect(validateRateLimit(with_({ rate })).errors["rateLimit.rate"], rate).toBeDefined();
    }
    expect(validateRateLimit(with_({ burst: "25" })).value?.burst).toBe(25);
    expect(validateRateLimit(with_({ burst: "10001" })).errors["rateLimit.burst"]).toBeDefined();
    expect(validateRateLimit(with_({ rate: "20000" })).errors["rateLimit.burst"]).toMatch(/set it/);
    expect(validateRateLimit(with_({ rate: "20000", burst: "100" })).value).toBeDefined();
  });

  it("keys on a header by its lower-case name and refuses connection headers", () => {
    expect(validateRateLimit(with_({ key: "header", header: " X-Api-Key " })).value).toEqual({
      rate: 10,
      periodSecs: 1,
      key: "header",
      header: "x-api-key",
    });
    for (const header of ["", "bad header", "x".repeat(65), "Host", "transfer-encoding"]) {
      expect(validateRateLimit(with_({ key: "header", header })).errors["rateLimit.header"], header).toBeDefined();
    }
  });

  it("sends an IPv6 prefix other than 64 for a client key", () => {
    expect(validateRateLimit(with_({ ipv6Prefix: "56" })).value?.ipv6Prefix).toBe(56);
    for (const ipv6Prefix of ["47", "129", ""]) {
      expect(validateRateLimit(with_({ ipv6Prefix })).errors["rateLimit.ipv6Prefix"]).toBeDefined();
    }
  });
});

describe("rateLimitValuesFrom", () => {
  it("shows the period in its largest whole unit and an equal burst as empty", () => {
    expect(rateLimitValuesFrom(policy())).toMatchObject({ periodAmount: "1", periodUnit: "m", burst: "" });
    expect(rateLimitValuesFrom(policy({ periodSecs: 3600, burst: 3 }))).toMatchObject({
      periodAmount: "1",
      periodUnit: "h",
      burst: "3",
    });
    expect(rateLimitValuesFrom(policy({ periodSecs: 90 }))).toMatchObject({ periodAmount: "90", periodUnit: "s" });
  });

  it("round-trips through the form", () => {
    const stored = policy({ key: "header", header: "x-customer", burst: 30, periodSecs: 300 });
    expect(validateRateLimit(rateLimitValuesFrom(stored)).value).toEqual({
      rate: 10,
      periodSecs: 300,
      burst: 30,
      key: "header",
      header: "x-customer",
    });
  });
});

describe("rateLimitSummary", () => {
  it("says the rate, the period, a different burst and a header key", () => {
    expect(rateLimitSummary(policy())).toBe("10 / min");
    expect(rateLimitSummary(policy({ periodSecs: 1, burst: 20 }))).toBe("10 / s, burst 20");
    expect(rateLimitSummary(policy({ periodSecs: 300, key: "header", header: "x-api-key" }))).toBe(
      "10 / 5 min by x-api-key",
    );
    expect(rateLimitSummary(policy({ periodSecs: 90 }))).toBe("10 / 90 s");
    expect(rateLimitSummary(policy({ periodSecs: 3600 }))).toBe("10 / h");
  });
});
