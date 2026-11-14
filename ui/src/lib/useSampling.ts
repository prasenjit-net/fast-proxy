// The sampling settings, overrides and status. A viewer may read them; the
// sampled requests themselves are an admin's (see the Samples page).
import { useQuery } from "@tanstack/react-query";
import { api } from "./api";
import type { SamplingOverride, SamplingView } from "./types";

export const SAMPLING_KEY = "sampling";

export function useSampling() {
  return useQuery({ queryKey: [SAMPLING_KEY], queryFn: api.sampling });
}

/** The override of a subsystem (`routeId` absent) or of a route, if there is one. */
export function overrideOf(
  view: SamplingView | undefined,
  target: { subsystemId: number } | { routeId: number },
): SamplingOverride | undefined {
  if (!view) return undefined;
  return view.overrides.find((entry) =>
    "routeId" in target
      ? entry.scope === "route" && entry.routeId === target.routeId
      : entry.scope === "subsystem" && entry.subsystemId === target.subsystemId,
  );
}
