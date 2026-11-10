// Small helpers for showing certificates.
import type { CertificateSource, CertificateState } from "./types";

export const EXPIRING_DAYS = 14;

const DAY_SECS = 86_400;

/** Whole days until expiry (negative once expired). */
export function daysLeft(secondsLeft: number): number {
  return Math.floor(secondsLeft / DAY_SECS);
}

function expiredLabel(daysAgo: number): string {
  if (daysAgo === 0) return "expired today";
  return `expired ${daysAgo} day${daysAgo === 1 ? "" : "s"} ago`;
}

/** "in 45 days", "tomorrow", "in 5 hours", "expired 3 days ago". */
export function expiryLabel(secondsLeft: number): string {
  if (secondsLeft <= 0) return expiredLabel(Math.floor(-secondsLeft / DAY_SECS));
  const days = daysLeft(secondsLeft);
  if (days >= 2) return `in ${days} days`;
  if (days === 1) return "tomorrow";
  return `in ${Math.max(1, Math.floor(secondsLeft / 3600))} hours`;
}

export const SOURCE_LABELS: Record<CertificateSource, string> = {
  uploaded: "uploaded",
  self_signed: "self-signed",
  acme: "ACME",
};

/** The tone a certificate state is shown in. */
export function stateTone(state: CertificateState): "ok" | "warn" | "err" {
  if (state === "ok") return "ok";
  return state === "expiring" ? "warn" : "err";
}

/** A development certificate is one the proxy generated: browsers will not trust it. */
export function isSelfSigned(source: CertificateSource): boolean {
  return source === "self_signed";
}

/** Reads a chosen file as text, in the browser; nothing is uploaded until the form is sent. */
export function readFileText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
    reader.onerror = () => reject(reader.error ?? new Error("The file could not be read"));
    reader.readAsText(file);
  });
}
