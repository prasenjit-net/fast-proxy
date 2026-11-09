// Code-based route tree (no file-based codegen — kept simple and fully
// readable in one file, which matters more than convention for a small,
// fixed set of routes in a template). `Layout` is the root route's
// component, so the sidebar/topbar shell wraps every page via <Outlet/>.
import { createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import Layout from "./components/Layout";
import ComponentsPage from "./pages/Components";
import DashboardPage from "./pages/Dashboard";
import NotFoundPage from "./pages/NotFound";
import SettingsPage from "./pages/Settings";

const rootRoute = createRootRoute({
  component: Layout,
});

const dashboardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: DashboardPage,
});

const componentsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/components",
  component: ComponentsPage,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});

const routeTree = rootRoute.addChildren([dashboardRoute, componentsRoute, settingsRoute]);

export const router = createRouter({
  routeTree,
  defaultNotFoundComponent: NotFoundPage,
});

// Registers the concrete router type globally so <Link to="…"> and
// useNavigate() are type-checked against the real route paths above.
declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
