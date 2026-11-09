import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Layout from "./Layout";

// Layout pulls in Sidebar (useConfig), Topbar -> ConnectionBadge (useLive)
// and ThemeToggle (useTheme). LiveContext's real provider opens a
// WebSocket in a useEffect, which jsdom has no server for — so these are
// mocked to keep the test hermetic and focused on the collapse mechanism.
vi.mock("../context/ConfigContext", () => ({
  useConfig: () => ({
    ui: { appName: "Test App", tagline: "Testing", repoUrl: null },
    version: "0.0.0",
  }),
}));
vi.mock("../context/LiveContext", () => ({
  useLive: () => ({ status: "online" }),
}));
vi.mock("../context/ThemeContext", () => ({
  useTheme: () => ({ mode: "light", setMode: vi.fn() }),
}));

// A minimal one-route test router — Layout is the root route's component
// (exactly as in src/router.tsx), with a single index child so
// useLocation()/<Outlet/> have something real to resolve against.
function buildTestRouter() {
  const rootRoute = createRootRoute({ component: Layout });
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <div>Page content</div>,
  });
  const routeTree = rootRoute.addChildren([indexRoute]);
  return createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/"] }) });
}

function renderLayout() {
  return render(<RouterProvider router={buildTestRouter()} />);
}

// "Dashboard" also appears as the Topbar's page-title <h1>, so the nav
// label needs to be queried within the sidebar's <nav> specifically.
function sidebarNavLabel(text: string): HTMLElement {
  const nav = document.querySelector("nav")!;
  return within(nav).getByText(text).closest("span")!;
}

function setMobile(isMobile: boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: isMobile && query.includes("max-width"),
    media: query,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
}

describe("Layout sidebar collapse", () => {
  beforeEach(() => {
    window.localStorage.clear();
    setMobile(false);
  });

  it("starts expanded (icon + text) when nothing is saved", async () => {
    renderLayout();
    await screen.findByLabelText("Toggle sidebar");
    expect(sidebarNavLabel("Dashboard").className).not.toContain("md:hidden");
  });

  it("collapses to an icon-only rail on hamburger click and persists it", async () => {
    const user = userEvent.setup();
    renderLayout();
    await screen.findByLabelText("Toggle sidebar");

    await user.click(screen.getByLabelText("Toggle sidebar"));

    expect(sidebarNavLabel("Dashboard").className).toContain("md:hidden");
    expect(window.localStorage.getItem("rt-sidebar")).toBe("collapsed");
  });

  it("expands again on a second hamburger click", async () => {
    const user = userEvent.setup();
    renderLayout();
    await screen.findByLabelText("Toggle sidebar");

    await user.click(screen.getByLabelText("Toggle sidebar"));
    await user.click(screen.getByLabelText("Toggle sidebar"));

    expect(sidebarNavLabel("Dashboard").className).not.toContain("md:hidden");
    expect(window.localStorage.getItem("rt-sidebar")).toBe("expanded");
  });

  it("restores a previously collapsed state on mount", async () => {
    window.localStorage.setItem("rt-sidebar", "collapsed");
    renderLayout();
    await screen.findByLabelText("Toggle sidebar");
    expect(sidebarNavLabel("Dashboard").className).toContain("md:hidden");
  });

  it("opens an overlay drawer instead of collapsing on mobile viewports", async () => {
    setMobile(true);
    const user = userEvent.setup();
    renderLayout();
    await screen.findByLabelText("Toggle sidebar");

    const aside = document.querySelector("aside")!;
    expect(aside.className).toContain("-translate-x-full");

    await user.click(screen.getByLabelText("Toggle sidebar"));

    expect(aside.className).toContain("translate-x-0");
    // Collapse state (desktop-only concept) must be untouched by the
    // mobile drawer toggle.
    expect(window.localStorage.getItem("rt-sidebar")).toBeNull();
  });
});
