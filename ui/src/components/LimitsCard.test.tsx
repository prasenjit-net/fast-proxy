import { screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { envelope, limitsStatus } from "../test/fixtures";
import { apiError, mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import { trafficSample } from "../test/traffic";
import LimitsCard from "./LimitsCard";

const live = vi.hoisted(() => ({ value: { traffic: null } as unknown }));
vi.mock("../context/LiveContext", () => ({ useLive: () => live.value }));

beforeEach(() => {
  live.value = { traffic: trafficSample({ activeConnections: 42 }) };
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("LimitsCard", () => {
  it("shows connections against their cap, the key table and nothing refused", async () => {
    mockApi({ "GET /api/limits": { body: envelope(limitsStatus()) } });
    renderPage(LimitsCard, { path: "/" });
    expect(await screen.findByText("42 of 10,000")).toBeInTheDocument();
    expect(screen.getByText("at most 256 per client")).toBeInTheDocument();
    expect(screen.getByText("0 of 100,000")).toBeInTheDocument();
    expect(screen.getByText("unlimited")).toBeInTheDocument();
    expect(screen.getAllByText("none")).toHaveLength(3);
    expect(screen.getByText("within limits")).toBeInTheDocument();
  });

  it("lists refusals by reason, skipping zeros, and warns when the key table fills", async () => {
    mockApi({
      "GET /api/limits": {
        body: envelope(
          limitsStatus({
            settings: { ...limitsStatus().settings, maxConnectionsPerClient: null, maxBodyBytes: 1_048_576 },
            rateLimitKeys: { used: 90, capacity: 100 },
            rateLimitTableFull: 3,
            requestsRefused: [
              { reason: "rate_limited", count: 12 },
              { reason: "uri_too_long", count: 0 },
              { reason: "something_new", count: 1 },
            ],
            connectionsRefused: [{ reason: "max_connections_per_client", count: 4 }],
            clientTimeouts: [{ reason: "first_byte", count: 2 }],
          }),
        ),
      },
    });
    renderPage(LimitsCard, { path: "/" });
    const requests = within(await screen.findByRole("list", { name: "Requests refused" }));
    expect(requests.getByText("over a rate limit (429)")).toBeInTheDocument();
    expect(requests.getByText("12")).toBeInTheDocument();
    expect(requests.getByText("something_new")).toBeInTheDocument();
    expect(requests.queryByText(/target too long/)).not.toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Connections refused" })).getByText("per-client cap")).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Clients timed out" })).getByText("sent nothing")).toBeInTheDocument();
    expect(screen.getByText("no cap per client")).toBeInTheDocument();
    expect(screen.getByText("1,048,576 bytes")).toBeInTheDocument();
    expect(screen.getByText("3 new clients let through while full")).toBeInTheDocument();
    expect(screen.getByText("key table filling")).toBeInTheDocument();
  });

  it("says when the limits cannot be read", async () => {
    mockApi({ "GET /api/limits": apiError(500, "INTERNAL", "boom") });
    renderPage(LimitsCard, { path: "/" });
    expect(await screen.findByText("The limits are unavailable.")).toBeInTheDocument();
  });
});
