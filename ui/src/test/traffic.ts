import type { Traffic } from "../lib/api";

/** A traffic sample with sensible defaults; override what a test cares about. */
export function trafficSample(overrides: Partial<Traffic> = {}): Traffic {
  return {
    timestampMs: 1_700_000_000_000,
    windowSecs: 2,
    total: { requestsTotal: 120, requestsPerSec: 12.5, errorRate: 0.25 },
    activeConnections: 3,
    activeRequests: 1,
    subsystems: [
      { name: "shop", requestsTotal: 100, requestsPerSec: 10, errorRate: 0.1 },
    ],
    ...overrides,
  };
}
