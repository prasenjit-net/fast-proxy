import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LiveProvider, useLive } from "./LiveContext";

vi.mock("./ToastContext", () => ({ useToast: () => ({ push: vi.fn() }) }));

let ticketCalls = 0;

/** Waits until the provider has asked for a ticket and opened its socket. */
async function socketOpened(): Promise<FakeSocket> {
  await waitFor(() => expect(FakeSocket.last).toBeDefined());
  return FakeSocket.last;
}

/** A WebSocket the test drives by hand. */
class FakeSocket {
  static last: FakeSocket;
  onopen: (() => void) | null = null;
  onmessage: ((message: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  readonly url: string;
  constructor(url: string | URL) {
    this.url = String(url);
    FakeSocket.last = this;
  }
  close() {}
  open() {
    this.onopen?.();
  }
  receive(event: unknown) {
    this.onmessage?.({ data: JSON.stringify(event) });
  }
}

function Probe() {
  const { status, traffic, history, activities } = useLive();
  return (
    <div>
      <span data-testid="status">{status}</span>
      <span data-testid="rps">{traffic ? traffic.total.requestsPerSec : "none"}</span>
      <span data-testid="history">{history.length}</span>
      <span data-testid="activities">{activities.map((a) => a.message).join("|")}</span>
    </div>
  );
}

// The exact shape the backend sends (see `Event::Traffic` in the admin crate).
const trafficEvent = (rps: number) => ({
  type: "traffic",
  data: {
    timestampMs: 1_700_000_000_000,
    windowSecs: 2.0,
    total: { requestsTotal: 100, requestsPerSec: rps, errorRate: 0.1 },
    activeConnections: 2,
    activeRequests: 1,
    subsystems: [{ name: "shop", requestsTotal: 100, requestsPerSec: rps, errorRate: 0.1 }],
  },
});

async function renderLive(client = new QueryClient()) {
  const view = render(
    <QueryClientProvider client={client}>
      <LiveProvider>
        <Probe />
      </LiveProvider>
    </QueryClientProvider>,
  );
  await socketOpened();
  return view;
}

describe("LiveProvider", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", FakeSocket);
    // Every connection first trades the signed-in token for a ticket.
    ticketCalls = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(() => {
        ticketCalls += 1;
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: `ticket-${ticketCalls}`, expiresInSecs: 30 }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("goes online when the socket opens", async () => {
    await renderLive();
    expect(screen.getByTestId("status")).toHaveTextContent("connecting");
    act(() => FakeSocket.last.open());
    expect(screen.getByTestId("status")).toHaveTextContent("online");
  });

  it("keeps the latest traffic sample and a history of them", async () => {
    await renderLive();
    expect(screen.getByTestId("rps")).toHaveTextContent("none");
    act(() => FakeSocket.last.receive(trafficEvent(4)));
    act(() => FakeSocket.last.receive(trafficEvent(9)));
    expect(screen.getByTestId("rps")).toHaveTextContent("9");
    expect(screen.getByTestId("history")).toHaveTextContent("2");
  });

  it("caps the history length", async () => {
    await renderLive();
    act(() => {
      for (let i = 0; i < 120; i++) FakeSocket.last.receive(trafficEvent(i));
    });
    expect(screen.getByTestId("history")).toHaveTextContent("90");
  });

  it("adds activity and hello events to the feed, newest first", async () => {
    await renderLive();
    act(() => FakeSocket.last.receive({ type: "hello", message: "Connected", timestampMs: 1 }));
    act(() =>
      FakeSocket.last.receive({
        type: "activity",
        kind: "config",
        message: "route created: / in shop",
        timestampMs: 2,
      }),
    );
    expect(screen.getByTestId("activities")).toHaveTextContent("route created: / in shop|Connected");
  });

  it("ignores messages it cannot parse", async () => {
    await renderLive();
    act(() => FakeSocket.last.onmessage?.({ data: "not json" }));
    expect(screen.getByTestId("rps")).toHaveTextContent("none");
    expect(screen.getByTestId("activities")).toHaveTextContent("");
  });

  it("refetches config data (but not the boot config) when the configuration changes", async () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await renderLive(client);
    act(() =>
      FakeSocket.last.receive({ type: "configChanged", version: 3, summary: "x", timestampMs: 1 }),
    );
    expect(invalidate).toHaveBeenCalledTimes(1);
    const { predicate } = invalidate.mock.calls[0][0] as unknown as {
      predicate: (query: { queryKey: unknown[] }) => boolean;
    };
    expect(predicate({ queryKey: ["subsystems"] })).toBe(true);
    expect(predicate({ queryKey: ["revisions"] })).toBe(true);
    expect(predicate({ queryKey: ["config"] })).toBe(false);
  });

  it("refetches endpoint status and the health summary when an endpoint changes state", async () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await renderLive(client);
    act(() =>
      FakeSocket.last.receive({
        type: "endpointState",
        subsystem: "shop",
        upstream: "backend",
        endpoint: "10.0.0.1:80",
        change: "ejected",
        detail: "ejection number 1 in a row",
        timestampMs: 1,
      }),
    );
    expect(invalidate).toHaveBeenCalledTimes(1);
    const { predicate } = invalidate.mock.calls[0][0] as unknown as {
      predicate: (query: { queryKey: unknown[] }) => boolean;
    };
    expect(predicate({ queryKey: ["upstream-status", 1, 5] })).toBe(true);
    expect(predicate({ queryKey: ["status"] })).toBe(true);
    expect(predicate({ queryKey: ["subsystems"] })).toBe(false);
    expect(predicate({ queryKey: ["config"] })).toBe(false);
  });

  it("refetches certificate data, the TLS state and the status card on a certState event", async () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await renderLive(client);
    act(() => FakeSocket.last.open());
    act(() =>
      FakeSocket.last.receive({ type: "certState", change: "uploaded", certificate: "shop", timestampMs: 5 }),
    );
    expect(invalidate).toHaveBeenCalledTimes(1);
    const { predicate } = invalidate.mock.calls[0][0] as unknown as {
      predicate: (query: { queryKey: unknown[] }) => boolean;
    };
    for (const key of ["certificates", "tls", "status", "subsystem", "ca-bundles"]) {
      expect(predicate({ queryKey: [key] }), key).toBe(true);
    }
    for (const key of ["config", "subsystems", "revisions"]) {
      expect(predicate({ queryKey: [key] }), key).toBe(false);
    }
  });

  it("opens the socket with a single-use ticket, never the token", async () => {
    await renderLive();
    expect(ticketCalls).toBe(1);
    expect(FakeSocket.last.url).toMatch(/^ws:\/\/[^/]+\/ws\?ticket=ticket-1$/);
    const calls = (fetch as unknown as { mock: { calls: [string, RequestInit][] } }).mock.calls;
    expect(calls[0][0]).toBe("/api/ws-ticket");
    expect(calls[0][1].method).toBe("POST");
  });

  it("is offline, and tries again later, when a ticket cannot be had", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ error: { code: "TOO_MANY_REQUESTS", message: "busy" } }), {
            status: 429,
          }),
        ),
      ),
    );
    render(
      <QueryClientProvider client={new QueryClient()}>
        <LiveProvider>
          <Probe />
        </LiveProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("status")).toHaveTextContent("offline"));
  });

  it("refetches what shows tokens and maintenance when either changes", async () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await renderLive(client);
    act(() =>
      FakeSocket.last.receive({ type: "tokenChanged", change: "revoked", token: "ci", timestampMs: 1 }),
    );
    act(() =>
      FakeSocket.last.receive({
        type: "maintenanceChanged",
        scope: "global",
        subsystem: null,
        route: null,
        active: true,
        expiresAtMs: null,
        timestampMs: 2,
      }),
    );
    expect(invalidate).toHaveBeenCalledTimes(2);
  });

  it("encodes a ticket that holds characters a URL cannot carry", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ ticket: "a b&c=d/e?f#g", expiresInSecs: 30 }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      ),
    );
    await renderLive();
    const url = new URL(FakeSocket.last.url);
    expect(url.pathname).toBe("/ws");
    expect([...url.searchParams.keys()]).toEqual(["ticket"]);
    expect(url.searchParams.get("ticket")).toBe("a b&c=d/e?f#g");
    expect(url.hash).toBe("");
  });
});
