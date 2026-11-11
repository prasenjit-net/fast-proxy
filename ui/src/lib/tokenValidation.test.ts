import { describe, expect, it } from "vitest";
import { tokenState, validateToken } from "./tokenValidation";

const values = { name: "ci-deploy", role: "viewer" as const, expiresInDays: "" };

describe("validateToken", () => {
  it("accepts a name, a role and no expiry", () => {
    expect(validateToken(values).value).toEqual({ name: "ci-deploy", role: "viewer" });
  });

  it("trims the name and sends an expiry as a number", () => {
    const result = validateToken({ ...values, name: "  ci  ", role: "admin", expiresInDays: "30" });
    expect(result.value).toEqual({ name: "ci", role: "admin", expiresInDays: 30 });
  });

  it("rejects an empty or over-long name and control characters", () => {
    for (const name of ["", "   ", "x".repeat(64), "a\nb"]) {
      expect(validateToken({ ...values, name }).errors.name, JSON.stringify(name)).toBeTruthy();
    }
    expect(validateToken({ ...values, name: "x".repeat(63) }).errors.name).toBeUndefined();
  });

  it("rejects an expiry outside 1 to 3650 days", () => {
    for (const expiresInDays of ["0", "-1", "3651", "1.5", "soon"]) {
      expect(validateToken({ ...values, expiresInDays }).errors.expiresInDays, expiresInDays).toBeTruthy();
    }
    expect(validateToken({ ...values, expiresInDays: "3650" }).errors.expiresInDays).toBeUndefined();
  });
});

describe("tokenState", () => {
  const now = 1_000;
  it("is revoked first, then expired, else active", () => {
    expect(tokenState({ revokedAtMs: 5, expiresAtMs: null }, now)).toBe("revoked");
    expect(tokenState({ revokedAtMs: 5, expiresAtMs: 10 }, now)).toBe("revoked");
    expect(tokenState({ revokedAtMs: null, expiresAtMs: 1_000 }, now)).toBe("expired");
    expect(tokenState({ revokedAtMs: null, expiresAtMs: 1_001 }, now)).toBe("active");
    expect(tokenState({ revokedAtMs: null, expiresAtMs: null }, now)).toBe("active");
  });
});
