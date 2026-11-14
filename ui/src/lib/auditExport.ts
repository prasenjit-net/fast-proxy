import { downloadAuthenticated } from "./download";

/** Downloads the audit log as NDJSON (every entry since `sinceMs`, or all). */
export function downloadAuditExport(sinceMs?: number): Promise<void> {
  return downloadAuthenticated(`/api/audit/export${sinceMs ? `?since=${sinceMs}` : ""}`, "audit.ndjson");
}
