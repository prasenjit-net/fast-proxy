// HTTP core of the API client. Every backend failure — network, HTTP
// status, or the server's JSON error envelope — is normalized into
// ApiError, which the toast layer renders as an error notification bubble
// and forms read `fields` from to show inline validation messages.

/** One invalid input named by the server (field names are camelCase). */
export interface FieldError {
  field: string;
  message: string;
}

export class ApiError extends Error {
  readonly code: string;
  readonly status: number;
  readonly fields: FieldError[];

  constructor(code: string, status: number, message: string, fields: FieldError[] = []) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
    this.fields = fields;
  }

  /** The server's message for `field`, if it named that field. */
  fieldMessage(field: string): string | undefined {
    return this.fields.find((entry) => entry.field === field)?.message;
  }
}

import { getToken, reportUnauthorized } from "./session";

interface ErrorBody {
  error?: { code?: string; message?: string; fields?: FieldError[] };
}

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const explicit = init?.headers as Record<string, string> | undefined;
  const token = getToken();
  let res: Response;
  try {
    res = await fetch(path, {
      ...init,
      headers: {
        "Content-Type": "application/json",
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...explicit,
      },
    });
  } catch {
    throw new ApiError("NETWORK", 0, "Cannot reach the server");
  }

  if (!res.ok) {
    // A 401 the sign-in form did not ask for (it sets its own header) means the
    // token was revoked or expired: sign the user out.
    if (res.status === 401 && !explicit?.Authorization) reportUnauthorized();
    // Prefer the backend's { error: { code, message, fields? } } envelope.
    let code = `HTTP_${res.status}`;
    let message = res.statusText || "Request failed";
    let fields: FieldError[] = [];
    try {
      const body = (await res.json()) as ErrorBody;
      if (body.error) {
        code = body.error.code ?? code;
        message = body.error.message ?? message;
        fields = body.error.fields ?? [];
      }
    } catch {
      /* body was not JSON — keep the status text */
    }
    throw new ApiError(code, res.status, message, fields);
  }

  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

/** JSON request options for a write. */
export function send(method: "POST" | "PUT" | "DELETE", body?: unknown): RequestInit {
  return { method, body: body === undefined ? undefined : JSON.stringify(body) };
}
