// A fetch mock for page tests: handlers are keyed by "METHOD /path" (query
// string included) and every call is recorded, so a test can assert both
// what the UI showed and exactly what it sent.
import { vi } from "vitest";

export interface RecordedCall {
  method: string;
  path: string;
  body: unknown;
  /** The request headers as sent, for tests of authentication. */
  headers: Record<string, string>;
}

export interface Reply {
  status?: number;
  body?: unknown;
}

type Handler = Reply | ((call: RecordedCall) => Reply);

export function mockApi(handlers: Record<string, Handler>) {
  const calls: RecordedCall[] = [];
  const fetchMock = vi.fn((path: string, init?: RequestInit) => {
    const call: RecordedCall = {
      method: init?.method ?? "GET",
      path,
      body: init?.body ? JSON.parse(init.body as string) : undefined,
      headers: (init?.headers as Record<string, string> | undefined) ?? {},
    };
    calls.push(call);
    const handler = handlers[`${call.method} ${call.path}`];
    if (!handler) {
      const error = { error: { code: "UNMOCKED", message: `no mock for ${call.method} ${call.path}` } };
      return Promise.resolve(new Response(JSON.stringify(error), { status: 500 }));
    }
    const reply = typeof handler === "function" ? handler(call) : handler;
    return Promise.resolve(
      new Response(JSON.stringify(reply.body ?? {}), {
        status: reply.status ?? 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  });
  vi.stubGlobal("fetch", fetchMock);

  return {
    calls,
    /** The calls made with `method` to a path starting with `path`. */
    called: (method: string, path: string) =>
      calls.filter((call) => call.method === method && call.path.startsWith(path)),
  };
}

/** A server error response in the backend's envelope shape. */
export function apiError(
  status: number,
  code: string,
  message: string,
  fields?: { field: string; message: string }[],
): Reply {
  return { status, body: { error: { code, message, status, fields } } };
}
