// The scopes in maintenance. Refetched at once when the server announces a
// change (LiveContext invalidates every query), and a scope that expires by
// itself drops out locally the moment its time passes (see `stillActive`).
import { useQuery } from "@tanstack/react-query";
import { api } from "./api";
import type { MaintenanceScope, MaintenanceTarget } from "./types";

export const MAINTENANCE_KEY = "maintenance";

/** Whether the scope has not yet reached its own expiry at `nowMs`. */
export function stillActive(scope: MaintenanceScope, nowMs: number): boolean {
  return scope.expiresAtMs === null || scope.expiresAtMs > nowMs;
}

/** Whether `scope` is the one `target` names. */
export function isScopeOf(scope: MaintenanceScope, target: MaintenanceTarget): boolean {
  switch (target.scope) {
    case "global":
      return scope.scope === "global";
    case "subsystem":
      return scope.scope === "subsystem" && scope.subsystemId === target.subsystemId;
    case "route":
      return scope.scope === "route" && scope.routeId === target.routeId;
  }
}

export function useMaintenance() {
  return useQuery({ queryKey: [MAINTENANCE_KEY], queryFn: api.maintenance });
}
