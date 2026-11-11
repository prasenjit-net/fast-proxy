import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, maintenanceScope } from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import MaintenanceBanner from "./MaintenanceBanner";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function renderBanner(scopes: ReturnType<typeof maintenanceScope>[]) {
  const api = mockApi({ "GET /api/maintenance": { body: envelope(scopes) } });
  const view = renderPage(MaintenanceBanner, { path: "/banner" });
  return { api, ...view };
}

describe("MaintenanceBanner", () => {
  it("is absent while nothing is in maintenance", async () => {
    const { api } = renderBanner([]);
    await waitFor(() => expect(api.called("GET", "/api/maintenance")).toHaveLength(1));
    expect(screen.queryByRole("status", { name: "Maintenance" })).not.toBeInTheDocument();
  });

  it("names what is closed, with a link to the details", async () => {
    renderBanner([
      maintenanceScope(),
      maintenanceScope({ scope: "subsystem", subsystemId: 1, subsystem: "shop" }),
      maintenanceScope({ scope: "route", subsystemId: 2, subsystem: "blog", routeId: 9, route: "/api" }),
    ]);
    const banner = await screen.findByRole("status", { name: "Maintenance" });
    expect(banner).toHaveTextContent("the whole proxy, subsystem shop and 1 more");
    expect(screen.getByRole("link", { name: "Details" })).toBeInTheDocument();
  });

  it("appears when the list refetches with a scope, and goes when it is cleared", async () => {
    const { api, client } = renderBanner([]);
    await waitFor(() => expect(api.called("GET", "/api/maintenance")).toHaveLength(1));
    expect(screen.queryByRole("status", { name: "Maintenance" })).not.toBeInTheDocument();

    mockApi({ "GET /api/maintenance": { body: envelope([maintenanceScope()]) } });
    await act(() => client.invalidateQueries());
    expect(await screen.findByRole("status", { name: "Maintenance" })).toBeInTheDocument();

    mockApi({ "GET /api/maintenance": { body: envelope([]) } });
    await act(() => client.invalidateQueries());
    await waitFor(() => expect(screen.queryByRole("status", { name: "Maintenance" })).not.toBeInTheDocument());
  });

  it("goes away by itself when the scope's time passes, with no refetch", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { api } = renderBanner([maintenanceScope({ expiresAtMs: Date.now() + 3000 })]);
    expect(await screen.findByRole("status", { name: "Maintenance" })).toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(4000));
    expect(screen.queryByRole("status", { name: "Maintenance" })).not.toBeInTheDocument();
    expect(api.called("GET", "/api/maintenance")).toHaveLength(1);
  });
});
