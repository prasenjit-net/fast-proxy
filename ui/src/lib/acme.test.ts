import { describe, expect, it } from "vitest";
import { acmeStateBadge, isFailing, relativeTime, skipReasonText } from "./acme";
import type { AcmeState, AcmeStatus, SkipReason } from "./types";

describe("acme helpers", () => {
  it("labels every state with a tone", () => {
    const states: AcmeState[] = ["idle", "pending", "valid", "renewing", "failed", "rate_limited", "inactive"];
    for (const state of states) expect(acmeStateBadge(state).label).not.toBe("");
    expect(acmeStateBadge("failed").tone).toBe("err");
    expect(acmeStateBadge("valid").tone).toBe("ok");
  });

  it("explains every skip reason", () => {
    const reasons: SkipReason[] = ["wildcard_needs_dns01", "single_label", "not_allowed", "too_many_names"];
    for (const reason of reasons) expect(skipReasonText(reason)).not.toBe("");
    expect(skipReasonText("not_allowed")).toMatch(/allowed_domains/);
  });

  it("calls failed and rate-limited statuses failing", () => {
    const status = (state: AcmeState) => ({ state }) as AcmeStatus;
    expect(isFailing(status("failed"))).toBe(true);
    expect(isFailing(status("rate_limited"))).toBe(true);
    expect(isFailing(status("valid"))).toBe(false);
  });

  it("words times relative to now", () => {
    expect(relativeTime(30_000, 0)).toBe("in less than a minute");
    expect(relativeTime(5 * 60_000, 0)).toBe("in 5m");
    expect(relativeTime(0, 3 * 3_600_000 + 60_000)).toBe("3h 1m ago");
    expect(relativeTime(3 * 86_400_000 + 4 * 3_600_000, 0)).toBe("in 3d 4h");
  });
});
