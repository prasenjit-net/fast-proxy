// Fetches /api/config via TanStack Query and gates the app on it:
// children render only after the server config has arrived, so every
// consumer can read it synchronously. Shows a splash while loading and a
// retry screen when the backend is unreachable.
import { useQuery } from "@tanstack/react-query";
import { createContext, useContext, useEffect, type ReactNode } from "react";
import Logo from "../components/Logo";
import { api, type ServerConfig } from "../lib/api";

const ConfigContext = createContext<ServerConfig | null>(null);

export function ConfigProvider({ children }: { children: ReactNode }) {
  const query = useQuery({
    queryKey: ["config"],
    queryFn: api.config,
    staleTime: Infinity, // only changes on a server restart, i.e. a hard reload anyway
  });

  useEffect(() => {
    if (query.data) document.title = query.data.ui.appName;
  }, [query.data]);

  if (query.isError) {
    const message =
      query.error instanceof Error ? query.error.message : String(query.error);
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-4 p-8 text-center">
        <Logo size={52} />
        <h1 className="text-lg font-semibold">Cannot reach the server</h1>
        <p className="max-w-md text-ink-muted">{message}</p>
        <button className="btn btn-primary" onClick={() => query.refetch()}>
          Retry
        </button>
      </div>
    );
  }

  if (!query.data) {
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-4 p-8">
        <Logo size={52} />
        <div className="spinner" aria-label="Loading" />
      </div>
    );
  }

  return (
    <ConfigContext.Provider value={query.data}>{children}</ConfigContext.Provider>
  );
}

export function useConfig(): ServerConfig {
  const ctx = useContext(ConfigContext);
  if (!ctx) throw new Error("useConfig must be used inside <ConfigProvider>");
  return ctx;
}
