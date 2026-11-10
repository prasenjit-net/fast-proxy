// The live state of one upstream group. Refetched every few seconds, and at
// once when the server pushes an `endpointState` event or a config change
// (see LiveContext and refreshConfigData).
import { useQuery } from "@tanstack/react-query";
import { api } from "./api";

const REFRESH_MS = 5_000;

/** Query key prefix shared by every upstream's status, for invalidation. */
export const UPSTREAM_STATUS_KEY = "upstream-status";

export function useUpstreamStatus(subsystemId: number, upstreamId: number) {
  return useQuery({
    queryKey: [UPSTREAM_STATUS_KEY, subsystemId, upstreamId],
    queryFn: () => api.upstreamStatus(subsystemId, upstreamId),
    refetchInterval: REFRESH_MS,
  });
}
