import { screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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
});
