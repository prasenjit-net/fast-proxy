// Client-side checks for creating a token; the server re-validates (names are
// 1 to 63 characters without control characters or leading or trailing
// space, and unique).
import type { FieldErrors, Validated } from "./policyValidation";
import type { Role, TokenInput } from "./types";

export interface TokenValues {
  name: string;
  role: Role;
  /** Days until it expires; empty means never. */
  expiresInDays: string;
}

/** What the expiry menu offers, in days; "" is never. */
export const EXPIRY_CHOICES: { value: string; label: string }[] = [
  { value: "", label: "Never" },
  { value: "7", label: "7 days" },
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "365", label: "1 year" },
];

export function validateToken(values: TokenValues): Validated<TokenInput> {
  const errors: FieldErrors = {};
  const name = values.name.trim();
  if (name === "" || [...name].length > 63) {
    errors.name = "Use 1 to 63 characters, such as ci-deploy";
  } else if (/[\u0000-\u001f\u007f]/.test(name)) {
    errors.name = "Do not use control characters";
  }
  let expiresInDays: number | undefined;
  if (values.expiresInDays !== "") {
    expiresInDays = Number(values.expiresInDays);
    if (!Number.isInteger(expiresInDays) || expiresInDays < 1 || expiresInDays > 3650) {
      errors.expiresInDays = "Choose between 1 and 3650 days";
    }
  }
  if (Object.keys(errors).length > 0) return { errors };
  return { errors, value: { name, role: values.role, ...(expiresInDays ? { expiresInDays } : {}) } };
}

/** `active`, `revoked` or `expired` at `nowMs`. */
export function tokenState(
  token: { revokedAtMs: number | null; expiresAtMs: number | null },
  nowMs: number,
): "active" | "revoked" | "expired" {
  if (token.revokedAtMs !== null) return "revoked";
  if (token.expiresAtMs !== null && token.expiresAtMs <= nowMs) return "expired";
  return "active";
}
