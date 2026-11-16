// How ACME states and skip reasons read in the UI.
import type { AcmeState, AcmeStatus, SkipReason } from "./types";

type Tone = "neutral" | "ok" | "warn" | "err" | "info";

/** The label and badge tone of an ACME state. */
export function acmeStateBadge(state: AcmeState): { label: string; tone: Tone } {
  switch (state) {
    case "valid":
      return { label: "certificate issued", tone: "ok" };
    case "pending":
      return { label: "ordering", tone: "info" };
    case "renewing":
      return { label: "renewing", tone: "info" };
    case "failed":
      return { label: "failed", tone: "err" };
    case "rate_limited":
      return { label: "rate limited", tone: "warn" };
    case "inactive":
      return { label: "inactive", tone: "warn" };
    case "idle":
      return { label: "nothing to order", tone: "neutral" };
  }
}

/** Why a host is not in the certificate, in words. */
export function skipReasonText(reason: SkipReason): string {
  switch (reason) {
    case "wildcard_needs_dns01":
      return "wildcards need DNS-01, which is not supported";
    case "single_label":
      return "a single-label name cannot be publicly certified";
    case "not_allowed":
      return "outside allowed_domains";
    case "too_many_names":
      return "more than 100 names";
  }
}

/** Whether a status needs attention (an order failed or the CA asked us to wait). */
export function isFailing(status: AcmeStatus): boolean {
  return status.state === "failed" || status.state === "rate_limited";
}

/** A future or past instant as words relative to `nowMs`: "in 3d 4h", "2h ago". */
export function relativeTime(atMs: number, nowMs: number): string {
  const diff = atMs - nowMs;
  const abs = Math.abs(diff);
  const minutes = Math.floor(abs / 60_000);
  let text: string;
  if (minutes < 1) text = "less than a minute";
  else if (minutes < 60) text = `${minutes}m`;
  else if (minutes < 48 * 60) text = `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  else text = `${Math.floor(minutes / 1440)}d ${Math.floor((minutes % 1440) / 60)}h`;
  return diff >= 0 ? `in ${text}` : `${text} ago`;
}
