import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LiveProvider, useLive } from "./LiveContext";

vi.mock("./ToastContext", () => ({ useToast: () => ({ push: vi.fn() }) }));

/** A WebSocket the test drives by hand. */
class FakeSocket {
  static last: FakeSocket;
  onopen: (() => void) | null = null;
  onmessage: ((message: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(public url: string) {
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

function renderLive(client = new QueryClient()) {
  return render(
    <QueryClientProvider client={client}>
      <LiveProvider>
        <Probe />
      </LiveProvider>
    </QueryClientProvider>,
  );
}

describe("LiveProvider", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", FakeSocket);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("goes online when the socket opens", () => {
    renderLive();
    expect(screen.getByTestId("status")).toHaveTextContent("connecting");
    act(() => FakeSocket.last.open());
    expect(screen.getByTestId("status")).toHaveTextContent("online");
    expect(FakeSocket.last.url).toMatch(/\/ws$/);
  });

  it("keeps the latest traffic sample and a history of them", () => {
    renderLive();
    expect(screen.getByTestId("rps")).toHaveTextContent("none");
    act(() => FakeSocket.last.receive(trafficEvent(4)));
    act(() => FakeSocket.last.receive(trafficEvent(9)));
    expect(screen.getByTestId("rps")).toHaveTextContent("9");
    expect(screen.getByTestId("history")).toHaveTextContent("2");
  });

  it("caps the history length", () => {
    renderLive();
    act(() => {
      for (let i = 0; i < 120; i++) FakeSocket.last.receive(trafficEvent(i));
    });
    expect(screen.getByTestId("history")).toHaveTextContent("90");
  });

  it("adds activity and hello events to the feed, newest first", () => {
    renderLive();
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

  it("ignores messages it cannot parse", () => {
    renderLive();
    act(() => FakeSocket.last.onmessage?.({ data: "not json" }));
    expect(screen.getByTestId("rps")).toHaveTextContent("none");
    expect(screen.getByTestId("activities")).toHaveTextContent("");
  });

  it("refetches config data (but not the boot config) when the configuration changes", () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    renderLive(client);
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

  it("refetches endpoint status and the health summary when an endpoint changes state", () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    renderLive(client);
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

  it("refetches certificate data, the TLS state and the status card on a certState event", () => {
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    renderLive(client);
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
});
