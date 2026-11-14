import { describe, expect, it } from "vitest";
import {
  baselineWords,
  isInherited,
  listText,
  NO_OVERRIDE,
  overrideSummary,
  overrideValuesOf,
  parseList,
  validateOverride,
  validateSampling,
  valuesOf,
} from "./samplingValidation";
import { samplingSettings } from "../test/fixtures";
import type { SamplingOverride } from "./types";

const settings = samplingSettings({
  extraHeaders: ["x-tenant"],
  redactQueryParams: ["sessionid"],
  redactBodyFields: ["iban", "pin"],
});

describe("the settings form", () => {
  it("shows the stored values in the units the form uses and turns them back unchanged", () => {
    const values = valuesOf(settings);
    expect(values.slowP99Factor).toBe("3");
    expect(values.oversizedRequestMiB).toBe("8");
    expect(values.maxMiB).toBe("256");
    expect(values.bodyRequestKiB).toBe("4");
    expect(values.redactBodyFields).toBe("iban\npin");
    const checked = validateSampling(values);
    expect(checked.errors).toEqual({});
    const { builtInRedaction: _b, updatedBy: _u, updatedAtMs: _t, ...input } = settings;
    expect(checked.value).toEqual(input);
  });

  it("accepts every bound and refuses one past it, naming the API's field", () => {
    const cases: [keyof ReturnType<typeof valuesOf>, string, string, string][] = [
      ["slowThresholdMs", "1", "3600000", "slowThresholdMs"],
      ["clientErrorsPerMin", "1", "1000000", "clientErrorsPerMin"],
      ["baselinePerMille", "0", "1000", "baselinePerMille"],
      ["ratePerSec", "1", "10000", "ratePerSec"],
      ["rateBurst", "1", "10000", "rateBurst"],
      ["stormBurst", "1", "100", "stormBurst"],
      ["stormIntervalSecs", "1", "3600", "stormIntervalSecs"],
      ["retentionDays", "1", "365", "retentionDays"],
      ["maxRows", "100", "10000000", "maxRows"],
      ["maxMiB", "1", "16384", "maxBytes"],
      ["bodyRequestKiB", "1", "64", "bodyRequestCap"],
      ["bodyResponseKiB", "1", "64", "bodyResponseCap"],
    ];
    for (const [field, low, high, apiField] of cases) {
      for (const ok of [low, high]) {
        expect(validateSampling({ ...valuesOf(settings), [field]: ok }).errors).toEqual({});
      }
      const below = String(Number(low) - 1);
      const above = String(Number(high) + 1);
      for (const bad of [below, above, "", "1.5", "x"]) {
        if (bad === below && Number(low) === 0) continue; // a baseline of -1 is bad too, below 0
        const checked = validateSampling({ ...valuesOf(settings), [field]: bad });
        expect(Object.keys(checked.errors), `${field}=${bad}`).toContain(apiField);
        expect(checked.value).toBeUndefined();
      }
    }
  });

  it("takes the p99 factor as a decimal between 1.1 and 100", () => {
    const with_ = (slowP99Factor: string) => validateSampling({ ...valuesOf(settings), slowP99Factor });
    expect(with_("2.5").value?.slowP99FactorX10).toBe(25);
    expect(with_("1.1").errors).toEqual({});
    expect(with_("100").errors).toEqual({});
    for (const bad of ["1", "100.1", "", "abc"]) {
      expect(Object.keys(with_(bad).errors)).toContain("slowP99FactorX10");
    }
  });

  it("checks the name lists", () => {
    const with_ = (field: string, text: string) =>
      validateSampling({ ...valuesOf(settings), [field]: text });
    expect(with_("redactHeaders", "X-Internal-Key\nx-other").errors).toEqual({});
    expect(with_("redactHeaders", "bad name").errors.redactHeaders).toMatch(/header name/);
    expect(with_("extraHeaders", "a:b").errors.extraHeaders).toBeDefined();
    expect(with_("redactQueryParams", "a=b").errors.redactQueryParams).toBeDefined();
    expect(with_("redactBodyFields", "has space").errors.redactBodyFields).toBeDefined();
    expect(with_("redactBodyFields", "x".repeat(65)).errors.redactBodyFields).toBeDefined();
    const sixtyFive = Array.from({ length: 65 }, (_, n) => `f${n}`).join("\n");
    expect(with_("redactBodyFields", sixtyFive).errors.redactBodyFields).toBe("At most 64 names");
    expect(with_("redactBodyFields", "a\n\n  b  \n").value?.redactBodyFields).toEqual(["a", "b"]);
  });

  it("reads lists one name per line", () => {
    expect(parseList(" a \n\nb\n")).toEqual(["a", "b"]);
    expect(listText(["a", "b"])).toBe("a\nb");
    expect(parseList("")).toEqual([]);
  });

  it("says what a baseline rate means in words", () => {
    expect(baselineWords(0)).toBe("none of the ordinary requests");
    expect(baselineWords(1)).toBe("about 1 in 1,000 ordinary requests");
    expect(baselineWords(10)).toBe("about 1 in 100 ordinary requests");
    expect(baselineWords(1000)).toBe("every request");
  });
});

const override = (changes: Partial<SamplingOverride> = {}): SamplingOverride => ({
  scope: "route",
  subsystemId: 1,
  subsystem: "shop",
  routeId: 7,
  route: "/api",
  enabled: null,
  captureBodies: null,
  baselinePerMille: null,
  slowThresholdMs: null,
  slowP99FactorX10: null,
  setBy: "ann",
  setAtMs: 1,
  ...changes,
});

describe("the override form", () => {
  it("starts inherited when there is no override", () => {
    expect(overrideValuesOf(undefined)).toEqual(NO_OVERRIDE);
    expect(isInherited(NO_OVERRIDE)).toBe(true);
  });

  it("reads an override into the form and back, sending only what is overridden", () => {
    const values = overrideValuesOf(override({ enabled: true, captureBodies: false, baselinePerMille: 25 }));
    expect(values).toEqual({ enabled: "on", captureBodies: "off", baselinePerMille: "25", slowThresholdMs: "" });
    expect(isInherited(values)).toBe(false);
    const checked = validateOverride(values, { routeId: 7 });
    expect(checked.value).toEqual({ routeId: 7, enabled: true, captureBodies: false, baselinePerMille: 25 });
  });

  it("sends only the target when nothing is overridden", () => {
    expect(validateOverride(NO_OVERRIDE, { subsystemId: 1 }).value).toEqual({ subsystemId: 1 });
  });

  it("checks the numbers it was given", () => {
    const bad = validateOverride({ ...NO_OVERRIDE, baselinePerMille: "5000", slowThresholdMs: "0" }, { routeId: 1 });
    expect(Object.keys(bad.errors).sort()).toEqual(["baselinePerMille", "slowThresholdMs"]);
    expect(validateOverride({ ...NO_OVERRIDE, baselinePerMille: "0" }, { routeId: 1 }).value?.baselinePerMille).toBe(0);
  });

  it("summarises what an override changes", () => {
    expect(overrideSummary(undefined)).toBe("inherits");
    expect(overrideSummary(override())).toBe("inherits");
    expect(
      overrideSummary(override({ enabled: false, captureBodies: true, baselinePerMille: 5, slowThresholdMs: 300 })),
    ).toBe("off, bodies on, baseline 5‰, slow 300 ms");
  });
});
