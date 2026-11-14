import { samplesExportQuery } from "./api";
import { downloadAuthenticated } from "./download";
import type { SampleFilters } from "./types";

/** Downloads every sample matching `filters` as NDJSON. */
export function downloadSamplesExport(filters: SampleFilters): Promise<void> {
  return downloadAuthenticated(`/api/samples/export?${samplesExportQuery(filters)}`, "samples.ndjson");
}
