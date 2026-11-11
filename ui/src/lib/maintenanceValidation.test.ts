import { describe, expect, it } from "vitest";
import {
  defaultMaintenanceValues,
  splitCidrs,
  validateMaintenance,
  type MaintenanceValues,
} from "./maintenanceValidation";

const values = (changes: Partial<MaintenanceValues> = {}): MaintenanceValues => ({
  ...defaultMaintenanceValues(),
  ...changes,
});

describe("validateMaintenance", () => {
  it("builds the smallest request from the defaults: a half-hour TTL and no bypass", () => {
    expect(validateMaintenance(values()).value).toEqual({
      message: "",
      ttlSecs: 1800,
      bypass: { cidrs: [] },
    });
  });

  it("leaves the TTL out for 'until I turn it off'", () => {
    const { value } = validateMaintenance(values({ duration: "forever" }));
    expect(value).toBeDefined();
    expect(value).not.toHaveProperty("ttlSecs");
  });

  it("turns custom minutes into seconds and rejects nonsense", () => {
    expect(validateMaintenance(values({ duration: "custom", customMinutes: "90" })).value?.ttlSecs).toBe(5400);
    for (const bad of ["", "0", "1.5", "abc", "43201"]) {
      const result = validateMaintenance(values({ duration: "custom", customMinutes: bad }));
      expect(result.errors.ttlSecs, bad).toBeDefined();
      expect(result.value).toBeUndefined();
    }
    expect(validateMaintenance(values({ duration: "custom", customMinutes: "43200" })).value?.ttlSecs).toBe(2_592_000);
  });

  it("checks retry-after against the server's 1 to 86400", () => {
    expect(validateMaintenance(values({ retryAfterSecs: "30" })).value?.retryAfterSecs).toBe(30);
    for (const bad of ["0", "86401", "x"]) {
      expect(validateMaintenance(values({ retryAfterSecs: bad })).errors.retryAfterSecs, bad).toBeDefined();
    }
  });

  it("sends a custom body only with its content type, and refuses an empty or oversized one", () => {
    const sent = validateMaintenance(values({ customBody: true, body: "<h1>Soon</h1>", contentType: "text/plain" }));
    expect(sent.value).toMatchObject({ body: "<h1>Soon</h1>", contentType: "text/plain" });
    expect(validateMaintenance(values({ customBody: true, body: "  " })).errors.body).toBeDefined();
    expect(validateMaintenance(values({ customBody: true, body: "é".repeat(8193) })).errors.body).toBeDefined();
    const off = validateMaintenance(values({ customBody: false, body: "ignored" }));
    expect(off.value).not.toHaveProperty("body");
  });

  it("splits networks on lines and commas, and checks their shape and count", () => {
    expect(splitCidrs("10.0.0.0/8\n 192.168.0.0/16, ,2001:db8::/32")).toEqual([
      "10.0.0.0/8",
      "192.168.0.0/16",
      "2001:db8::/32",
    ]);
    expect(validateMaintenance(values({ cidrs: "10.0.0.0/8" })).value?.bypass.cidrs).toEqual(["10.0.0.0/8"]);
    expect(validateMaintenance(values({ cidrs: "10.0.0.1" })).errors["bypass.cidrs"]).toBeDefined();
    const many = Array.from({ length: 33 }, (_, i) => `10.0.${i}.0/24`).join("\n");
    expect(validateMaintenance(values({ cidrs: many })).errors["bypass.cidrs"]).toMatch(/at most 32/);
  });

  it("needs a header name for a secret, a valid name, and a long enough secret", () => {
    expect(validateMaintenance(values({ secret: "x".repeat(16) })).errors["bypass.header"]).toBeDefined();
    expect(validateMaintenance(values({ headerName: "bad name" })).errors["bypass.header"]).toBeDefined();
    expect(
      validateMaintenance(values({ headerName: "X-Bypass", secret: "short" })).errors["bypass.secret"],
    ).toBeDefined();
    expect(validateMaintenance(values({ headerName: "X-Bypass" })).value?.bypass).toEqual({
      cidrs: [],
      header: "X-Bypass",
    });
    const secret = "s".repeat(16);
    expect(validateMaintenance(values({ headerName: " X-Bypass ", secret })).value?.bypass).toEqual({
      cidrs: [],
      header: "X-Bypass",
      secret,
    });
  });

  it("bounds the message in characters", () => {
    expect(validateMaintenance(values({ message: "é".repeat(1024) })).value).toBeDefined();
    expect(validateMaintenance(values({ message: "é".repeat(1025) })).errors.message).toBeDefined();
  });
});
