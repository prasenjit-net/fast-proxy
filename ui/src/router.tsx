// Code-based route tree (no file-based codegen — kept simple and fully
// readable in one file, which matters more than convention for a small,
// fixed set of routes in a template). `Layout` is the root route's
// component, so the sidebar/topbar shell wraps every page via <Outlet/>.
import { createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import Layout from "./components/Layout";
import DashboardPage from "./pages/Dashboard";
import NotFoundPage from "./pages/NotFound";
import ResolverPage from "./pages/Resolver";
import RevisionsPage from "./pages/Revisions";
import SettingsPage from "./pages/Settings";
import SubsystemDetailPage from "./pages/SubsystemDetail";
import SubsystemsPage from "./pages/Subsystems";

const rootRoute = createRootRoute({
  component: Layout,
});

const dashboardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: DashboardPage,
});

const subsystemsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/subsystems",
  component: SubsystemsPage,
});

const subsystemDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/subsystems/$id",
  component: SubsystemDetailPage,
});

const revisionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/revisions",
  component: RevisionsPage,
});

const resolverRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/resolver",
  component: ResolverPage,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});

const routeTree = rootRoute.addChildren([
  dashboardRoute,
  subsystemsRoute,
  subsystemDetailRoute,
  revisionsRoute,
  resolverRoute,
  settingsRoute,
]);

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
