import { QueryClientProvider } from "@tanstack/react-query";
import { ReactQueryDevtools } from "@tanstack/react-query-devtools";
import { RouterProvider } from "@tanstack/react-router";
import ErrorBoundary from "./components/ErrorBoundary";
import { ConfigProvider } from "./context/ConfigContext";
import { SessionProvider } from "./context/SessionContext";
import { LiveProvider } from "./context/LiveContext";
import { ThemeProvider } from "./context/ThemeContext";
import { ToastProvider } from "./context/ToastContext";
import { queryClient } from "./lib/queryClient";
import { router } from "./router";

// Provider order matters: toasts have no dependencies; the query client
// backs ConfigProvider's own useQuery (and every other data hook in the
// app), so it sits above; the session gates everything behind sign-in (the
// config itself needs a token); config gates the rest of the app (splash
// screen until loaded); the theme reads the server default; the live socket
// reports problems via toasts.
export default function App() {
  return (
    <ErrorBoundary>
      <ToastProvider>
        <QueryClientProvider client={queryClient}>
          <SessionProvider>
            <ConfigProvider>
              <ThemeProvider>
                <LiveProvider>
                  <RouterProvider router={router} />
                </LiveProvider>
              </ThemeProvider>
            </ConfigProvider>
          </SessionProvider>
          <ReactQueryDevtools initialIsOpen={false} />
        </QueryClientProvider>
      </ToastProvider>
    </ErrorBoundary>
  );
}
