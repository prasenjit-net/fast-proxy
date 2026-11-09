// Typed API client. Every backend failure — network, HTTP status, or the
// server's JSON error envelope — is normalized into ApiError, which the
// toast layer renders as an error notification bubble.

export interface UiConfig {
  appName: string;
  tagline: string;
  defaultTheme: string;
  repoUrl?: string | null;
}

export interface ServerConfig {
  ui: UiConfig;
  version: string;
  startedAtMs: number;
}

export interface Metrics {
  cpu: number;
  memory: number;
  requestsTotal: number;
  requestsPerMin: number;
  wsClients: number;
  uptimeSecs: number;
  timestampMs: number;
}

export class ApiError extends Error {
  readonly code: string;
  readonly status: number;

  constructor(code: string, status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      ...init,
      headers: {
        "Content-Type": "application/json",
        ...(init?.headers as Record<string, string> | undefined),
      },
    });
  } catch {
    throw new ApiError("NETWORK", 0, "Cannot reach the server");
  }

  if (!res.ok) {
    // Prefer the backend's { error: { code, message } } envelope.
    let code = `HTTP_${res.status}`;
    let message = res.statusText || "Request failed";
    try {
      const body = (await res.json()) as {
        error?: { code?: string; message?: string };
      };
      if (body.error) {
        code = body.error.code ?? code;
        message = body.error.message ?? message;
      }
    } catch {
      /* body was not JSON — keep the status text */
    }
    throw new ApiError(code, res.status, message);
  }

  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

export const api = {
  config: () => request<ServerConfig>("/api/config"),
  health: () => request<{ status: string; version: string }>("/api/health"),
  metrics: () => request<Metrics>("/api/metrics"),
};
