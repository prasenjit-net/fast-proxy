import { screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  endpointStatus,
  envelope,
  maintenanceScope,
  route,
  subsystemDetail,
  subsystemSummary,
  upstream,
  upstreamStatus,
} from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import RoutesOverviewPage from "./RoutesOverview";

afterEach(() => {
  vi.unstubAllGlobals();
});

const shop = () => subsystemDetail();
const blog = () =>
  subsystemDetail({
    id: 2,
    name: "blog",
    upstreams: [upstream({ id: 8, name: "web" })],
    routes: [route({ id: 9, prefix: "/posts", upstreamGroupId: 8, enabled: false })],
  });

function renderOverview(scopes: ReturnType<typeof maintenanceScope>[] = []) {
  mockApi({
    "GET /api/subsystems": {
      body: envelope([subsystemSummary(), subsystemSummary({ id: 2, name: "blog" })]),
    },
    "GET /api/subsystems/1": { body: envelope(shop()) },
    "GET /api/subsystems/2": { body: envelope(blog()) },
    "GET /api/maintenance": { body: envelope(scopes) },
    "GET /api/subsystems/1/upstreams/5/status": {
      body: envelope(upstreamStatus([endpointStatus(), endpointStatus({ id: 12, effective: "unhealthy" })])),
    },
    "GET /api/subsystems/2/upstreams/8/status": {
      body: envelope(upstreamStatus([endpointStatus({ id: 21, effective: "ejected" })])),
    },
  });
  renderPage(RoutesOverviewPage, { path: "/routes" });
}

const rowsOf = (heading: string) => {
  const section = screen.getByRole("heading", { name: heading }).closest("section")!;
  return within(section).getAllByRole("row").slice(1);
};

describe("RoutesOverviewPage", () => {
  it("lists every route of every subsystem with its upstream, enabled state and link", async () => {
    renderOverview();
    await screen.findByText("/posts");
    const rows = rowsOf("Routes");
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByRole("link", { name: "shop" })).toHaveAttribute("href", "/subsystems/1");
    expect(within(rows[0]).getByText("/api")).toBeInTheDocument();
    expect(within(rows[0]).getByText("backend")).toBeInTheDocument();
    expect(within(rows[2]).getByRole("link", { name: "blog" })).toHaveAttribute("href", "/subsystems/2");
    expect(within(rows[2]).getByText("web")).toBeInTheDocument();
    expect(within(rows[2]).getByText("Off")).toBeInTheDocument();
  });

  it("shows each group's health from its live status", async () => {
    renderOverview();
    expect(await screen.findAllByText("1/2 healthy")).not.toHaveLength(0);
    expect((await screen.findAllByText("0/1 healthy")).length).toBeGreaterThan(0);
    const groups = rowsOf("Upstream groups");
    expect(groups).toHaveLength(2);
    expect(within(groups[0]).getByText("backend")).toBeInTheDocument();
    expect(within(groups[0]).getByText("Round robin")).toBeInTheDocument();
  });

  it("marks what is in maintenance and by which scope", async () => {
    renderOverview([
      maintenanceScope({ scope: "route", subsystemId: 1, subsystem: "shop", routeId: 7, route: "/api" }),
      maintenanceScope({ scope: "subsystem", subsystemId: 2, subsystem: "blog" }),
    ]);
    await screen.findByText("/posts");
    await screen.findByText("maintenance (route)");
    const rows = rowsOf("Routes");
    expect(within(rows[0]).getByText("maintenance (route)")).toBeInTheDocument();
    expect(within(rows[1]).queryByText(/maintenance \(/)).not.toBeInTheDocument();
    expect(within(rows[2]).getByText("maintenance (subsystem)")).toBeInTheDocument();
  });

  it("marks everything when the whole proxy is in maintenance", async () => {
    renderOverview([maintenanceScope()]);
    await screen.findByText("/posts");
    await screen.findAllByText("maintenance (whole proxy)");
    expect(screen.getAllByText("maintenance (whole proxy)")).toHaveLength(5);
  });

  it("has no control that changes anything", async () => {
    renderOverview();
    await screen.findByText("/posts");
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });
});
