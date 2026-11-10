import { describe, expect, it } from "vitest";
import {
  parseNames,
  validateCaBundle,
  validateCertificateUpload,
  validateHstsMaxAge,
  validateMaterialName,
  validateSelfSigned,
} from "./tlsValidation";

const cert = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
const key = "-----BEGIN PRIVATE KEY-----\nBBBB\n-----END PRIVATE KEY-----";

describe("validateMaterialName", () => {
  it("accepts names the server accepts and rejects the rest", () => {
    for (const good of ["a", "shop", "wild.card_1", "a".repeat(63)]) {
      expect(validateMaterialName(good), good).toBeUndefined();
    }
    for (const bad of ["", "-a", "a-", "a b", "a/b", "a".repeat(64)]) {
      expect(validateMaterialName(bad), bad).toBeDefined();
    }
  });
});

describe("validateCertificateUpload", () => {
  it("passes a name, a certificate and a key, trimming them", () => {
    const result = validateCertificateUpload({ name: " shop ", certPem: `\n${cert}\n`, keyPem: ` ${key} ` });
    expect(result.errors).toEqual({});
    expect(result.value).toEqual({ name: "shop", certPem: cert, keyPem: key });
  });

  it.each(["RSA PRIVATE KEY", "EC PRIVATE KEY"])("accepts a %s", (label) => {
    const pem = `-----BEGIN ${label}-----\nX\n-----END ${label}-----`;
    expect(validateCertificateUpload({ name: "a", certPem: cert, keyPem: pem }).value).toBeDefined();
  });

  it("names every missing piece", () => {
    const result = validateCertificateUpload({ name: "", certPem: "nope", keyPem: "nope" });
    expect(Object.keys(result.errors).sort()).toEqual(["certPem", "keyPem", "name"]);
    expect(result.value).toBeUndefined();
  });

  it("explains that an encrypted key is not accepted", () => {
    const encrypted = "-----BEGIN ENCRYPTED PRIVATE KEY-----\nX\n-----END ENCRYPTED PRIVATE KEY-----";
    expect(validateCertificateUpload({ name: "a", certPem: cert, keyPem: encrypted }).errors.keyPem).toMatch(
      /passphrase/,
    );
  });

  it("never puts the pasted text into an error", () => {
    const result = validateCertificateUpload({ name: "a", certPem: cert, keyPem: "SECRET-TEXT" });
    expect(JSON.stringify(result.errors)).not.toContain("SECRET-TEXT");
  });
});

describe("validateCaBundle", () => {
  it("needs a name and at least one certificate", () => {
    expect(validateCaBundle({ name: "corp", pem: cert }).value).toEqual({ name: "corp", pem: cert });
    expect(Object.keys(validateCaBundle({ name: "", pem: "x" }).errors).sort()).toEqual(["name", "pem"]);
  });
});

describe("self-signed names", () => {
  it("splits on commas and spaces", () => {
    expect(parseNames("a.example.com, *.b.example.com  c.example.com")).toEqual([
      "a.example.com",
      "*.b.example.com",
      "c.example.com",
    ]);
    expect(parseNames("  ")).toEqual([]);
  });

  it("needs a name and one host", () => {
    expect(validateSelfSigned({ name: "dev", names: "a.example.com" }).value).toEqual({
      name: "dev",
      names: ["a.example.com"],
    });
    expect(Object.keys(validateSelfSigned({ name: "", names: "" }).errors).sort()).toEqual(["name", "names"]);
  });
});

describe("validateHstsMaxAge", () => {
  it("accepts whole seconds up to two years", () => {
    expect(validateHstsMaxAge("0")).toBeUndefined();
    expect(validateHstsMaxAge("63072000")).toBeUndefined();
    expect(validateHstsMaxAge("63072001")).toBeDefined();
    expect(validateHstsMaxAge("1.5")).toBeDefined();
    expect(validateHstsMaxAge("")).toBeDefined();
  });
});
