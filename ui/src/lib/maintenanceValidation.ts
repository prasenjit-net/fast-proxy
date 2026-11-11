// Form values and client-side checks for turning maintenance on. They mirror
// the server's rules (api/maintenance.rs); field names are the API's dotted
// camelCase paths, shared with server errors.
import { wholeNumber, type FieldErrors, type Validated } from "./policyValidation";
import type { MaintenanceInput } from "./types";

/** The longest TTL the server accepts: 30 days. */
export const MAX_TTL_SECS = 2_592_000;
/** An operator-chosen bypass secret must be at least this long. */
export const MIN_SECRET_LEN = 16;
const MAX_MESSAGE_CHARS = 1024;
const MAX_BODY_BYTES = 16 * 1024;
const MAX_CIDRS = 32;
const MAX_RETRY_AFTER_SECS = 86_400;

/** How long maintenance lasts: a preset, forever, or a custom number of minutes. */
export const DURATION_CHOICES: { value: string; label: string }[] = [
  { value: "600", label: "10 minutes" },
  { value: "1800", label: "30 minutes" },
  { value: "3600", label: "1 hour" },
  { value: "14400", label: "4 hours" },
  { value: "custom", label: "Custom (minutes)" },
  { value: "forever", label: "Until I turn it off" },
];

export const BODY_TYPES = ["text/html", "text/plain", "application/json"] as const;

export interface MaintenanceValues {
  message: string;
  /** A value of DURATION_CHOICES. */
  duration: string;
  customMinutes: string;
  retryAfterSecs: string;
  customBody: boolean;
  body: string;
  contentType: string;
  /** One network per line or comma. */
  cidrs: string;
  headerName: string;
  /** Empty: the server generates one and shows it once. */
  secret: string;
}

export function defaultMaintenanceValues(): MaintenanceValues {
  return {
    message: "",
    duration: "1800",
    customMinutes: "",
    retryAfterSecs: "",
    customBody: false,
    body: "",
    contentType: "text/html",
    cidrs: "",
    headerName: "",
    secret: "",
  };
}

// An HTTP header name is a token: letters, digits and these marks.
const HEADER_NAME = /^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,64}$/;
// A rough shape check; the server parses the network for real.
const CIDR_SHAPE = /^[0-9a-fA-F:.]+\/\d{1,3}$/;

function ttlSecs(values: MaintenanceValues, errors: FieldErrors): number | undefined {
  if (values.duration === "forever") return undefined;
  if (values.duration !== "custom") return Number(values.duration);
  const minutes = wholeNumber(values.customMinutes);
  if (minutes === null || minutes < 1 || minutes * 60 > MAX_TTL_SECS) {
    errors.ttlSecs = "Enter whole minutes from 1 to 43200 (30 days)";
    return undefined;
  }
  return minutes * 60;
}

/** The networks typed, one per line or comma separated, blanks dropped. */
export function splitCidrs(raw: string): string[] {
  return raw
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "");
}

/** Checks the values and builds the turn-on request. */
export function validateMaintenance(values: MaintenanceValues): Validated<MaintenanceInput> {
  const errors: FieldErrors = {};
  const ttl = ttlSecs(values, errors);

  let retryAfterSecs: number | undefined;
  if (values.retryAfterSecs.trim() !== "") {
    const secs = wholeNumber(values.retryAfterSecs);
    if (secs === null || secs < 1 || secs > MAX_RETRY_AFTER_SECS) {
      errors.retryAfterSecs = `Enter whole seconds from 1 to ${MAX_RETRY_AFTER_SECS}, or leave it empty`;
    } else {
      retryAfterSecs = secs;
    }
  }

  const cidrs = splitCidrs(values.cidrs);
  if (cidrs.some((cidr) => !CIDR_SHAPE.test(cidr))) {
    errors["bypass.cidrs"] = "Use networks such as 10.0.0.0/8 or 2001:db8::/32, one per line";
  } else if (cidrs.length > MAX_CIDRS) {
    errors["bypass.cidrs"] = `List at most ${MAX_CIDRS} networks`;
  }

  const headerName = values.headerName.trim();
  const secret = values.secret.trim();
  if (headerName !== "" && !HEADER_NAME.test(headerName)) {
    errors["bypass.header"] = "Use a header name such as X-Maintenance-Bypass";
  }
  if (secret !== "" && headerName === "") {
    errors["bypass.header"] = "Name the header the secret goes in";
  }
  if (secret !== "" && secret.length < MIN_SECRET_LEN) {
    errors["bypass.secret"] = `Use at least ${MIN_SECRET_LEN} characters, or leave it empty to generate one`;
  }

  if ([...values.message].length > MAX_MESSAGE_CHARS) {
    errors.message = `Use at most ${MAX_MESSAGE_CHARS} characters`;
  }
  if (values.customBody && values.body.trim() === "") {
    errors.body = "Write the page to show, or turn the custom page off";
  } else if (values.customBody && new TextEncoder().encode(values.body).length > MAX_BODY_BYTES) {
    errors.body = "The page must be at most 16 KiB";
  }

  if (Object.keys(errors).length > 0) return { errors };
  return {
    errors,
    value: {
      message: values.message.trim(),
      ...(values.customBody ? { body: values.body, contentType: values.contentType } : {}),
      ...(retryAfterSecs === undefined ? {} : { retryAfterSecs }),
      ...(ttl === undefined ? {} : { ttlSecs: ttl }),
      bypass: {
        cidrs,
        ...(headerName === "" ? {} : { header: headerName }),
        ...(secret === "" ? {} : { secret }),
      },
    },
  };
}
