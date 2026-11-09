import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

  it("shows placeholders until the first traffic sample arrives", () => {
    render(<DashboardPage />);
    expect(screen.getByText("Requests / s")).toBeInTheDocument();
    expect(screen.getAllByText("—")).toHaveLength(4);
    expect(screen.getByText("waiting for data")).toBeInTheDocument();
    expect(screen.getByText(/waiting for live data/i)).toBeInTheDocument();
  });

  it("shows live traffic numbers once a sample has arrived", () => {
    const sample = trafficSample({
      total: { requestsTotal: 4200, requestsPerSec: 3.25, errorRate: 0.025 },
      activeConnections: 7,
      activeRequests: 2,
    });
    live.value = { status: "online", traffic: sample, history: [sample, sample], activities: [] };
    render(<DashboardPage />);

    const card = (label: string) =>
      within(screen.getByText(label).closest(".card") as HTMLElement);
    expect(card("Requests / s").getByText("3.3")).toBeInTheDocument();
    expect(card("Requests / s").getByText("4,200 total")).toBeInTheDocument();
    expect(card("Error rate").getByText("2.5%")).toBeInTheDocument();
    expect(card("Connections").getByText("7")).toBeInTheDocument();
    expect(card("In flight").getByText("2")).toBeInTheDocument();
  });

  it("lists server activity", () => {
    live.value = {
      status: "online",
      traffic: null,
      history: [],
      activities: [{ kind: "config", message: "route created: / in shop", timestampMs: Date.now() }],
    };
    render(<DashboardPage />);
    expect(screen.getByText("route created: / in shop")).toBeInTheDocument();
  });
});
