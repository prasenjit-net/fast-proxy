import { describe, expect, it } from "vitest";
import { formatNumber, formatUptime, timeAgo } from "./format";

describe("formatUptime", () => {
  it("shows seconds under a minute", () => {
    expect(formatUptime(0)).toBe("0s");
    expect(formatUptime(45)).toBe("45s");
  });

  it("shows minutes and seconds under an hour", () => {
    expect(formatUptime(90)).toBe("1m 30s");
  });

  it("shows hours and minutes under a day", () => {
    expect(formatUptime(3_725)).toBe("1h 2m");
  });

  it("shows days and hours at or beyond a day", () => {
    expect(formatUptime(90_000)).toBe("1d 1h");
  });
});

describe("formatNumber", () => {
  it("adds thousands separators and drops decimals", () => {
    expect(formatNumber(1284)).toBe("1,284");
    expect(formatNumber(12.9)).toBe("13");
  });
});

describe("timeAgo", () => {
  it("says 'just now' for very recent timestamps", () => {
    expect(timeAgo(Date.now() - 2_000)).toBe("just now");
  });

  it("formats seconds, minutes, and hours ago", () => {
    expect(timeAgo(Date.now() - 30_000)).toBe("30s ago");
    expect(timeAgo(Date.now() - 5 * 60_000)).toBe("5m ago");
    expect(timeAgo(Date.now() - 3 * 3_600_000)).toBe("3h ago");
  });

  it("never goes negative for clock skew (future timestamps)", () => {
    expect(timeAgo(Date.now() + 10_000)).toBe("just now");
  });
});
