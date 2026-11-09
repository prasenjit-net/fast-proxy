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
import { ToastProvider } from "../context/ToastContext";

type Page = () => ReactElement | null;

const STUBS: Record<string, Page> = {
  "/": () => <div>dashboard page</div>,
  "/subsystems": () => <div>subsystems page</div>,
  "/subsystems/$id": () => <div>subsystem detail page</div>,
  "/revisions": () => <div>revisions page</div>,
  "/resolver": () => <div>resolver page</div>,
};

export function renderPage(
  component: Page,
  { path, entry = path }: { path: string; entry?: string },
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
        <RouterProvider router={router} />
      </ToastProvider>
    </QueryClientProvider>,
  );
  return { ...view, router, client };
}
