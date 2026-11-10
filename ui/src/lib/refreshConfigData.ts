// After any change to the configuration every list and detail that shows
// it may be stale (the revision history and status too), so refetch all
// queries except the server's boot config, which only changes on a restart.
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { UPSTREAM_STATUS_KEY } from "./useUpstreamStatus";

export function refreshConfigData(client: QueryClient): Promise<void> {
  return client.invalidateQueries({ predicate: (query) => query.queryKey[0] !== "config" });
}

/** Refetches the live endpoint status and the upstream health summary. */
export function refreshEndpointStatus(client: QueryClient): Promise<void> {
  return client.invalidateQueries({
    predicate: (query) => query.queryKey[0] === UPSTREAM_STATUS_KEY || query.queryKey[0] === "status",
  });
}

/** A function that refreshes everything derived from the configuration. */
export function useRefreshConfigData(): () => Promise<void> {
  const client = useQueryClient();
  return useCallback(() => refreshConfigData(client), [client]);
}
