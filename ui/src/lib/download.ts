// Some downloads are authenticated, so a plain link cannot fetch them: the page
// fetches the file with the token and hands the browser the bytes.
import { ApiError } from "./http";
import { getToken } from "./session";

/** Fetches `path` with the signed-in token and saves the answer as `filename`. */
export async function downloadAuthenticated(path: string, filename: string): Promise<void> {
  const token = getToken();
  let response: Response;
  try {
    response = await fetch(path, { headers: token ? { Authorization: `Bearer ${token}` } : {} });
  } catch {
    throw new ApiError("NETWORK", 0, "Cannot reach the server");
  }
  if (!response.ok) {
    throw new ApiError(`HTTP_${response.status}`, response.status, "The export was refused");
  }
  const url = URL.createObjectURL(await response.blob());
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.append(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
