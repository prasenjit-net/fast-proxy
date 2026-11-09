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

describe("LiveProvider", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", FakeSocket);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("goes online when the socket opens", () => {
    render(
      <LiveProvider>
        <Probe />
      </LiveProvider>,
    );
    expect(screen.getByTestId("status")).toHaveTextContent("connecting");
    act(() => FakeSocket.last.open());
    expect(screen.getByTestId("status")).toHaveTextContent("online");
    expect(FakeSocket.last.url).toMatch(/\/ws$/);
  });

  it("keeps the latest traffic sample and a history of them", () => {
    render(
      <LiveProvider>
        <Probe />
      </LiveProvider>,
    );
    expect(screen.getByTestId("rps")).toHaveTextContent("none");
    act(() => FakeSocket.last.receive(trafficEvent(4)));
    act(() => FakeSocket.last.receive(trafficEvent(9)));
    expect(screen.getByTestId("rps")).toHaveTextContent("9");
    expect(screen.getByTestId("history")).toHaveTextContent("2");
  });

  it("caps the history length", () => {
    render(
      <LiveProvider>
        <Probe />
      </LiveProvider>,
    );
    act(() => {
      for (let i = 0; i < 120; i++) FakeSocket.last.receive(trafficEvent(i));
    });
    expect(screen.getByTestId("history")).toHaveTextContent("90");
  });

  it("adds activity and hello events to the feed, newest first", () => {
    render(
      <LiveProvider>
        <Probe />
      </LiveProvider>,
    );
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

  it("ignores messages it cannot parse and events it does not handle", () => {
    render(
      <LiveProvider>
        <Probe />
      </LiveProvider>,
    );
    act(() => FakeSocket.last.onmessage?.({ data: "not json" }));
    act(() =>
      FakeSocket.last.receive({ type: "configChanged", version: 3, summary: "x", timestampMs: 1 }),
    );
    expect(screen.getByTestId("rps")).toHaveTextContent("none");
    expect(screen.getByTestId("activities")).toHaveTextContent("");
  });
});
