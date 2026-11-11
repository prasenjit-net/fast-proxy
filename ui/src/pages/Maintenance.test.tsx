import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, maintenanceScope } from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import MaintenancePage from "./Maintenance";

afterEach(() => {
  vi.unstubAllGlobals();
});

function renderMaintenance(scopes: ReturnType<typeof maintenanceScope>[], role: "admin" | "viewer" = "admin") {
  const api = mockApi({ "GET /api/maintenance": { body: envelope(scopes) } });
  renderPage(MaintenancePage, { path: "/maintenance", role });
  return api;
}

describe("MaintenancePage", () => {
  it("says so when nothing is in maintenance", async () => {
    renderMaintenance([]);
    expect(await screen.findByText("Nothing is in maintenance")).toBeInTheDocument();
  });

  it("lists every active scope with what it covers, its message, countdown, bypass and who set it", async () => {
    renderMaintenance([
      maintenanceScope({ expiresAtMs: null, bypass: { cidrs: ["10.0.0.0/8"], header: "X-Bypass", secretSet: true } }),
      maintenanceScope({
        scope: "subsystem",
        subsystemId: 1,
        subsystem: "shop",
        message: "Deploying",
        expiresAtMs: Date.now() + 90_000,
        setBy: "alice",
      }),
      maintenanceScope({ scope: "route", subsystemId: 1, subsystem: "shop", routeId: 7, route: "/api" }),
    ]);
    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByText("the whole proxy")).toBeInTheDocument();
    expect(within(rows[0]).getByText(/until turned off/)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/10\.0\.0\.0\/8 · header X-Bypass/)).toBeInTheDocument();
    expect(within(rows[1]).getByRole("link", { name: "subsystem shop" })).toHaveAttribute("href", "/subsystems/1");
    expect(within(rows[1]).getByText("Deploying")).toBeInTheDocument();
    expect(within(rows[1]).getByText(/ends in 1:(2\d|30)/)).toBeInTheDocument();
    expect(within(rows[1]).getByText(/by alice/)).toBeInTheDocument();
    expect(within(rows[2]).getByText("route /api of shop")).toBeInTheDocument();
  });

  it("leaves out a scope whose time has passed", async () => {
    renderMaintenance([maintenanceScope({ expiresAtMs: Date.now() - 1000 })]);
    expect(await screen.findByText("Nothing is in maintenance")).toBeInTheDocument();
  });

  it("gives an administrator the control for the whole proxy", async () => {
    renderMaintenance([]);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn on maintenance for the whole proxy/i }));
    expect(within(screen.getByRole("dialog")).getByRole("button", { name: /turn on for the whole proxy/i })).toBeInTheDocument();
  });

  it("shows a viewer everything and no button", async () => {
    renderMaintenance([maintenanceScope()], "viewer");
    await screen.findAllByText("the whole proxy");
    await waitFor(() => expect(screen.getAllByText("in maintenance").length).toBeGreaterThan(0));
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });
});
