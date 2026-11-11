// Renders a page the way the app does: inside a memory router, a query
// client with no retries, and the real toast provider (so tests can assert
// on the notification bubbles). Routes the page may navigate to are stubbed.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { SessionContext, type SessionValue } from "../context/SessionContext";
import { ToastProvider } from "../context/ToastContext";
import type { Role } from "../lib/types";

type Page = () => ReactElement | null;

const STUBS: Record<string, Page> = {
  "/": () => <div>dashboard page</div>,
  "/subsystems": () => <div>subsystems page</div>,
  "/subsystems/$id": () => <div>subsystem detail page</div>,
  "/revisions": () => <div>revisions page</div>,
  "/resolver": () => <div>resolver page</div>,
};

/** A signed-in session for page tests; the default is an administrator. */
export function sessionOf(role: Role = "admin", name = "test-admin"): SessionValue {
  return {
    who: { name, role, tokenExpiresAtMs: null, protectedChannel: true, auth: "required" },
    canWrite: role === "admin",
    isAdmin: role === "admin",
    signOut: () => undefined,
  };
}

export function renderPage(
  component: Page,
  { path, entry = path, role = "admin" }: { path: string; entry?: string; role?: Role },
) {
  const rootRoute = createRootRoute({ component: Outlet });
  const routes = Object.entries({ ...STUBS, [path]: component }).map(([routePath, routeComponent]) =>
    createRoute({ getParentRoute: () => rootRoute, path: routePath, component: routeComponent }),
  );
  const router = createRouter({
    routeTree: rootRoute.addChildren(routes),
    history: createMemoryHistory({ initialEntries: [entry] }),
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 0 } } });
  const view = render(
    <QueryClientProvider client={client}>
      <ToastProvider>
        <SessionContext.Provider value={sessionOf(role)}>
          <RouterProvider router={router} />
        </SessionContext.Provider>
      </ToastProvider>
    </QueryClientProvider>,
  );
  return { ...view, router, client };
}
