// The export is an authenticated download, so a plain link cannot fetch it:
// the page fetches it with the token and hands the browser the bytes.
import { ApiError } from "./http";
import { getToken } from "./session";

/** Downloads the audit log as NDJSON (every entry since `sinceMs`, or all). */
export async function downloadAuditExport(sinceMs?: number): Promise<void> {
  const token = getToken();
  let response: Response;
  try {
    response = await fetch(`/api/audit/export${sinceMs ? `?since=${sinceMs}` : ""}`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
    });
  } catch {
    throw new ApiError("NETWORK", 0, "Cannot reach the server");
  }
  if (!response.ok) {
    throw new ApiError(`HTTP_${response.status}`, response.status, "The export was refused");
  }
  const url = URL.createObjectURL(await response.blob());
  const link = document.createElement("a");
  link.href = url;
  link.download = "audit.ndjson";
  document.body.append(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
