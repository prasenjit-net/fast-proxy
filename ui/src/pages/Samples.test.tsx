import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { downloadSamplesExport } from "../lib/samplesExport";
import type { SamplePage } from "../lib/types";
import { envelope, sampleDetail, sampleSummary, samplingView } from "../test/fixtures";
import { mockApi, type Reply } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import SamplesPage from "./Samples";

vi.mock("../lib/samplesExport", () => ({ downloadSamplesExport: vi.fn().mockResolvedValue(undefined) }));

const page = (data = [sampleSummary()], nextBefore: number | null = null): Reply => ({
  body: { data, nextBefore, version: 3 } satisfies SamplePage,
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.mocked(downloadSamplesExport).mockClear();
});

describe("SamplesPage", () => {
  it("lists samples with why, what, status and client", async () => {
    mockApi({
      "GET /api/samples?limit=50": page([
        sampleSummary({ id: 2, status: 502, client: "203.0.113.7" }),
        sampleSummary({ id: 1, primary: "slow", triggers: ["slow"], status: 200, totalMs: 3200 }),
      ]),
    });
    renderPage(SamplesPage, { path: "/samples" });
    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("Upstream unreachable")).toBeInTheDocument();
    expect(within(rows[0]).getByText("+1")).toBeInTheDocument();
    expect(within(rows[0]).getByText("502")).toBeInTheDocument();
    expect(within(rows[0]).getByText("203.0.113.7")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Slow")).toBeInTheDocument();
    expect(within(rows[1]).getByText("3.2s")).toBeInTheDocument();
  });

  it("shows a sample's detail on demand", async () => {
    mockApi({
      "GET /api/samples?limit=50": page(),
      "GET /api/samples/1": { body: envelope(sampleDetail()) },
    });
    const user = userEvent.setup();
    renderPage(SamplesPage, { path: "/samples" });
    await user.click(await screen.findByRole("button", { name: "Details" }));
    expect(await screen.findByText(/GET \/api\/orders\?token=%5Bredacted%5D/)).toBeInTheDocument();
    expect(screen.getByText("10.0.0.1:80: connect_refused")).toBeInTheDocument();
    expect(screen.getByText("not kept: bodies are off")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByText("10.0.0.1:80: connect_refused")).not.toBeInTheDocument();
  });

  it("sends the filters to the server", async () => {
    const api = mockApi({ "GET /api/samples?limit=50": page(), "GET /api/samples": page() });
    const user = userEvent.setup();
    renderPage(SamplesPage, { path: "/samples" });
    await screen.findByRole("button", { name: "Details" });
    await user.selectOptions(screen.getByLabelText("Why"), "slow");
    await user.selectOptions(screen.getByLabelText("Status"), "5");
    await user.type(screen.getByLabelText("Route"), "/api");
    await user.type(screen.getByLabelText("Slower than (ms)"), "500");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(api.calls.some((call) => call.path.includes("trigger=slow") && call.path.includes("statusClass=5") && call.path.includes("route=%2Fapi") && call.path.includes("minMs=500"))).toBe(true),
    );
  });

  it("explains an empty list, and says when filters hide everything", async () => {
    mockApi({ "GET /api/samples?limit=50": page([]), "GET /api/samples?limit=50&trigger=slow": page([]) });
    const user = userEvent.setup();
    renderPage(SamplesPage, { path: "/samples" });
    expect(await screen.findByText("No unusual requests kept yet")).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("Why"), "slow");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    expect(await screen.findByText("Nothing matches")).toBeInTheDocument();
  });

  it("loads older samples when there are more", async () => {
    const api = mockApi({
      "GET /api/samples?limit=50": page([sampleSummary({ id: 2 })], 2),
      "GET /api/samples?limit=50&before=2": page([sampleSummary({ id: 1, client: "198.51.100.1" })]),
    });
    renderPage(SamplesPage, { path: "/samples" });
    await userEvent.setup().click(await screen.findByRole("button", { name: "Load older samples" }));
    expect(await screen.findByText("198.51.100.1")).toBeInTheDocument();
    expect(api.called("GET", "/api/samples?limit=50&before=2")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Load older samples" })).not.toBeInTheDocument();
  });

  it("exports with the applied filters", async () => {
    mockApi({ "GET /api/samples?limit=50": page() });
    renderPage(SamplesPage, { path: "/samples" });
    await userEvent.setup().click(await screen.findByRole("button", { name: "Export NDJSON" }));
    expect(downloadSamplesExport).toHaveBeenCalledWith({});
  });

  it("deletes everything only after a confirmation", async () => {
    const api = mockApi({
      "GET /api/samples?limit=50": page(),
      "DELETE /api/samples": { body: { deleted: 12 } },
    });
    const user = userEvent.setup();
    renderPage(SamplesPage, { path: "/samples" });
    await user.click(await screen.findByRole("button", { name: "Delete all" }));
    expect(api.called("DELETE", "/api/samples")).toHaveLength(0);
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Delete all" }));
    expect(await screen.findByText("Deleted 12 samples")).toBeInTheDocument();
    expect(api.called("DELETE", "/api/samples")).toHaveLength(1);
  });

  it("keeps the list when the confirmation is cancelled", async () => {
    const api = mockApi({ "GET /api/samples?limit=50": page() });
    const user = userEvent.setup();
    renderPage(SamplesPage, { path: "/samples" });
    await user.click(await screen.findByRole("button", { name: "Delete all" }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(api.called("DELETE", "/api/samples")).toHaveLength(0);
  });

  it("shows a load error", async () => {
    mockApi({});
    renderPage(SamplesPage, { path: "/samples" });
    expect(await screen.findByText(/Could not load the samples/)).toBeInTheDocument();
  });

  it("tells a viewer the page is for administrators, without asking for a sample", async () => {
    const api = mockApi({ "GET /api/sampling": { body: envelope(samplingView()) } });
    renderPage(SamplesPage, { path: "/samples", role: "viewer" });
    expect(await screen.findByText("Samples are for administrators")).toBeInTheDocument();
    expect(await screen.findByText(/42 requests were kept and 3 were dropped/)).toBeInTheDocument();
    expect(api.calls.some((call) => call.path.startsWith("/api/samples"))).toBe(false);
  });
});
