// Cheap client-side checks for the TLS forms. They only catch obvious
// mistakes (a missing BEGIN line, an empty name); the server parses the PEM
// and is the authority, and its field errors are shown the same way.
import type { CertificateUpload } from "./types";
import type { FieldErrors, Validated } from "./policyValidation";

const MATERIAL_NAME = /^[A-Za-z0-9]([A-Za-z0-9._-]{0,61}[A-Za-z0-9])?$/;
const MAX_HSTS_SECS = 63_072_000;

/** Whether `text` holds a PEM block with the given label. */
function hasPem(text: string, label: string): boolean {
  return text.includes(`-----BEGIN ${label}-----`) && text.includes(`-----END ${label}-----`);
}

/** A private key label: PKCS#8, PKCS#1 (RSA) or SEC1 (EC), but not an encrypted one. */
function hasPrivateKey(text: string): boolean {
  return ["PRIVATE KEY", "RSA PRIVATE KEY", "EC PRIVATE KEY"].some((label) => hasPem(text, label));
}

export function validateMaterialName(name: string, field = "name"): string | undefined {
  return MATERIAL_NAME.test(name.trim())
    ? undefined
    : `${field === "name" ? "Use" : "Use"} 1 to 63 letters, digits, '.', '_' or '-', starting and ending with a letter or digit`;
}

export function validateCertificateUpload(values: CertificateUpload): Validated<CertificateUpload> {
  const errors: FieldErrors = {};
  const name = validateMaterialName(values.name);
  if (name) errors.name = name;
  if (!hasPem(values.certPem, "CERTIFICATE")) {
    errors.certPem = "Paste the certificate chain in PEM form (it starts with -----BEGIN CERTIFICATE-----)";
  }
  if (values.keyPem.includes("ENCRYPTED")) {
    errors.keyPem = "This key is passphrase-protected; upload an unencrypted key (it is encrypted at rest here)";
  } else if (!hasPrivateKey(values.keyPem)) {
    errors.keyPem = "Paste the private key in PEM form (it starts with -----BEGIN PRIVATE KEY-----)";
  }
  if (Object.keys(errors).length > 0) return { errors };
  return { errors, value: { name: values.name.trim(), certPem: values.certPem.trim(), keyPem: values.keyPem.trim() } };
}

export function validateCaBundle(values: { name: string; pem: string }): Validated<{ name: string; pem: string }> {
  const errors: FieldErrors = {};
  const name = validateMaterialName(values.name);
  if (name) errors.name = name;
  if (!hasPem(values.pem, "CERTIFICATE")) {
    errors.pem = "Paste at least one CA certificate in PEM form (-----BEGIN CERTIFICATE-----)";
  }
  if (Object.keys(errors).length > 0) return { errors };
  return { errors, value: { name: values.name.trim(), pem: values.pem.trim() } };
}

/** Names for a generated certificate: comma or whitespace separated host names. */
export function parseNames(raw: string): string[] {
  return raw
    .split(/[\s,]+/)
    .map((name) => name.trim())
    .filter((name) => name !== "");
}

export function validateSelfSigned(values: { name: string; names: string }): Validated<{ name: string; names: string[] }> {
  const errors: FieldErrors = {};
  const name = validateMaterialName(values.name);
  if (name) errors.name = name;
  const names = parseNames(values.names);
  if (names.length === 0) errors.names = "Enter at least one host name, such as shop.example.com or *.example.com";
  if (Object.keys(errors).length > 0) return { errors };
  return { errors, value: { name: values.name.trim(), names } };
}

/** HSTS lifetimes the form offers, in seconds. */
export const HSTS_PRESETS: { secs: number; label: string }[] = [
  { secs: 300, label: "5 minutes (testing)" },
  { secs: 86_400, label: "1 day" },
  { secs: 2_592_000, label: "30 days" },
  { secs: 31_536_000, label: "1 year" },
  { secs: MAX_HSTS_SECS, label: "2 years" },
];

export function validateHstsMaxAge(raw: string): string | undefined {
  if (!/^\d+$/.test(raw.trim())) return "Enter a whole number of seconds";
  const value = Number(raw.trim());
  return value > MAX_HSTS_SECS ? "At most 63072000 seconds (two years)" : undefined;
}
