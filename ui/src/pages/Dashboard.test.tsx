import { screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { envelope } from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import { trafficSample } from "../test/traffic";
import DashboardPage from "./Dashboard";

const live = vi.hoisted(() => ({
  value: { status: "online", traffic: null, history: [], activities: [] } as unknown,
}));
vi.mock("../context/LiveContext", () => ({ useLive: () => live.value }));

describe("DashboardPage", () => {
  beforeEach(() => {
    live.value = { status: "online", traffic: null, history: [], activities: [] };
  });

  it("shows placeholders until the first traffic sample arrives", async () => {
    renderPage(DashboardPage, { path: "/" });
    expect(await screen.findByText("Requests / s")).toBeInTheDocument();
    expect(screen.getAllByText("—")).toHaveLength(4);
    expect(screen.getByText("waiting for data")).toBeInTheDocument();
    expect(screen.getByText(/waiting for live data/i)).toBeInTheDocument();
  });

  it("shows live traffic numbers once a sample has arrived", async () => {
    const sample = trafficSample({
      total: { requestsTotal: 4200, requestsPerSec: 3.25, errorRate: 0.025 },
      activeConnections: 7,
      activeRequests: 2,
    });
    live.value = { status: "online", traffic: sample, history: [sample, sample], activities: [] };
    renderPage(DashboardPage, { path: "/" });

    await screen.findByRole("table");
    // The stat card labels are spans; the by-subsystem table has same-named headers.
    const card = (label: string) =>
      within(screen.getByText(label, { selector: "span" }).closest(".card") as HTMLElement);
    expect(card("Requests / s").getByText("3.3")).toBeInTheDocument();
    expect(card("Requests / s").getByText("4,200 total")).toBeInTheDocument();
    expect(card("Error rate").getByText("2.5%")).toBeInTheDocument();
    expect(card("Connections").getByText("7")).toBeInTheDocument();
    expect(card("In flight").getByText("2")).toBeInTheDocument();
  });

  it("lists server activity", async () => {
    live.value = {
      status: "online",
      traffic: null,
      history: [],
      activities: [{ kind: "config", message: "route created: / in shop", timestampMs: Date.now() }],
    };
    renderPage(DashboardPage, { path: "/" });
    expect(await screen.findByText("route created: / in shop")).toBeInTheDocument();
  });

  it("breaks traffic down by subsystem, marking unmatched traffic", async () => {
    const sample = trafficSample({
      subsystems: [
        { name: "_unmatched", requestsTotal: 5, requestsPerSec: 0.5, errorRate: 0 },
        { name: "shop", requestsTotal: 1000, requestsPerSec: 12.5, errorRate: 0.05 },
      ],
    });
    live.value = { status: "online", traffic: sample, history: [sample, sample], activities: [] };
    renderPage(DashboardPage, { path: "/" });

    const table = within(await screen.findByRole("table"));
    expect(table.getByRole("link", { name: "shop" })).toBeInTheDocument();
    expect(table.getByText("_unmatched")).toHaveAttribute("title", "Requests that matched no subsystem");
    expect(table.queryByRole("link", { name: "_unmatched" })).not.toBeInTheDocument();
    expect(table.getByText("5.0%")).toBeInTheDocument();
    expect(table.getByText("1,000")).toBeInTheDocument();
  });

  it("explains an empty subsystem table", async () => {
    const sample = trafficSample({ subsystems: [] });
    live.value = { status: "online", traffic: sample, history: [sample, sample], activities: [] };
    renderPage(DashboardPage, { path: "/" });
    expect(await screen.findByText("No traffic has reached the proxy yet.")).toBeInTheDocument();
  });

  describe("upstream health card", () => {
    const status = (upstreams: unknown) =>
      mockApi({
        "GET /api/status": {
          body: envelope({
            listeners: [],
            database: "x.db",
            subsystems: { total: 1, enabled: 1 },
            upstreams,
            startedAtMs: 0,
            uptimeSecs: 1,
          }),
        },
      });

    it("lists the endpoints that are out of rotation with the reason", async () => {
      status({
        total: 3,
        healthy: 1,
        unhealthy: [
          { subsystem: "shop", upstream: "backend", endpoint: "10.0.0.2:80", reason: "failed health checks" },
          { subsystem: "shop", upstream: "backend", endpoint: "10.0.0.3:80", reason: "draining" },
        ],
      });
      renderPage(DashboardPage, { path: "/" });

      expect(await screen.findByText("1 of 3 healthy")).toBeInTheDocument();
      expect(screen.getByText("10.0.0.2:80")).toBeInTheDocument();
      expect(screen.getByText("failed health checks")).toBeInTheDocument();
      expect(screen.getByText("draining")).toBeInTheDocument();
    });

    it("colours the summary by how many endpoints are in rotation", async () => {
      for (const [healthy, tone] of [[2, "text-ok"], [1, "text-warn"], [0, "text-err"]] as const) {
        status({ total: 2, healthy, unhealthy: [] });
        const { unmount } = renderPage(DashboardPage, { path: "/" });
        expect((await screen.findByText(`${healthy} of 2 healthy`)).className).toContain(tone);
        unmount();
      }
    });

    it("says so when every endpoint is taking requests", async () => {
      status({ total: 2, healthy: 2, unhealthy: [] });
      renderPage(DashboardPage, { path: "/" });
      expect(await screen.findByText("2 of 2 healthy")).toBeInTheDocument();
      expect(screen.getByText("Every endpoint is taking requests.")).toBeInTheDocument();
    });

    it("says so when no endpoint is configured", async () => {
      status({ total: 0, healthy: 0, unhealthy: [] });
      renderPage(DashboardPage, { path: "/" });
      expect(await screen.findByText("No endpoints are configured yet.")).toBeInTheDocument();
    });

    it("says the status is unavailable when it cannot be fetched", async () => {
      mockApi({});
      renderPage(DashboardPage, { path: "/" });
      expect(await screen.findByText("Status is unavailable.")).toBeInTheDocument();
    });
  });
});
