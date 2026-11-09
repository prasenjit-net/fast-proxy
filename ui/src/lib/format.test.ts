import { describe, expect, it } from "vitest";
import {
  formatDurationMs,
  formatNumber,
  formatPercent,
  formatRate,
  formatUptime,
  timeAgo,
} from "./format";

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

describe("formatRate", () => {
  it("keeps one decimal below 10 per second", () => {
    expect(formatRate(0)).toBe("0.0");
    expect(formatRate(2.456)).toBe("2.5");
    expect(formatRate(9.94)).toBe("9.9");
  });

  it("rounds to whole numbers from 10 per second", () => {
    expect(formatRate(10)).toBe("10");
    expect(formatRate(1234.6)).toBe(formatNumber(1234.6));
  });
});

describe("formatPercent", () => {
  it("shows exact zero as 0%", () => {
    expect(formatPercent(0)).toBe("0%");
  });

  it("keeps small non-zero values visible", () => {
    expect(formatPercent(0.0004)).toBe("<1%");
  });

  it("shows one decimal below 10% and none above", () => {
    expect(formatPercent(0.025)).toBe("2.5%");
    expect(formatPercent(0.25)).toBe("25%");
    expect(formatPercent(1)).toBe("100%");
  });
});

describe("formatDurationMs", () => {
  it("uses milliseconds, seconds, or minutes as fits", () => {
    expect(formatDurationMs(250)).toBe("250ms");
    expect(formatDurationMs(5000)).toBe("5s");
    expect(formatDurationMs(1500)).toBe("1.5s");
    expect(formatDurationMs(120_000)).toBe("2m");
    expect(formatDurationMs(90_000)).toBe("1.5m");
  });
});
