import { Link } from "@tanstack/react-router";
import { IconAlertTriangle } from "../icons";
import { stillActive, useMaintenance } from "../lib/useMaintenance";
import { useNow } from "../lib/useNow";
import type { MaintenanceScope } from "../lib/types";

/** A short name for what a scope covers. */
export function scopeLabel(scope: MaintenanceScope): string {
  if (scope.scope === "global") return "the whole proxy";
  if (scope.scope === "subsystem") return `subsystem ${scope.subsystem ?? scope.subsystemId}`;
  return `route ${scope.route ?? scope.routeId} of ${scope.subsystem ?? scope.subsystemId}`;
}

/**
 * Shown under the top bar while anything is in maintenance, so nobody forgets
 * a closed route. It follows the live list and drops a scope the moment its
 * own expiry passes.
 */
export default function MaintenanceBanner() {
  const query = useMaintenance();
  const now = useNow();
  const active = (query.data?.data ?? []).filter((scope) => stillActive(scope, now));
  if (active.length === 0) return null;
  const covers = active.map(scopeLabel);
  const shown = covers.slice(0, 2).join(", ");
  const more = covers.length > 2 ? ` and ${covers.length - 2} more` : "";
  return (
    <div
      role="status"
      aria-label="Maintenance"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-warn-soft px-4 py-2 text-[0.85rem] text-warn md:px-6"
    >
      <IconAlertTriangle size={16} />
      <span className="font-medium">
        Maintenance is on for {shown}
        {more}
      </span>
      <Link to="/maintenance" className="ml-auto underline">
        Details
      </Link>
    </div>
  );
}
