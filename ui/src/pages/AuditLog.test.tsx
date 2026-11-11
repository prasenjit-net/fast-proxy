import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { downloadAuditExport } from "../lib/auditExport";
import type { AuditPage } from "../lib/types";
import { auditEntry } from "../test/fixtures";
import { apiError, mockApi, type Reply } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import AuditLogPage from "./AuditLog";

vi.mock("../lib/auditExport", () => ({ downloadAuditExport: vi.fn().mockResolvedValue(undefined) }));

const page = (data = [auditEntry()], nextBefore: number | null = null): Reply => ({
  body: { data, nextBefore, version: 3 } satisfies AuditPage,
});

function renderAudit(replies: Record<string, Reply>, role: "admin" | "viewer" = "admin") {
  const api = mockApi({ "GET /api/audit?limit=50": page(), ...replies });
  renderPage(AuditLogPage, { path: "/audit", role });
  return api;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.mocked(downloadAuditExport).mockClear();
});

describe("AuditLogPage", () => {
  it("lists entries with who, what, the outcome and where it came from", async () => {
    renderAudit({
      "GET /api/audit?limit=50": page([
        auditEntry({ id: 3, actor: "ann", action: "route.create", target: "shop", outcome: "ok" }),
        auditEntry({ id: 2, actor: "anonymous", action: "auth.failed", target: "/api/status", outcome: "denied", source: "203.0.113.9" }),
      ]),
    });
    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("ann")).toBeInTheDocument();
    expect(within(rows[0]).getByText("route.create")).toBeInTheDocument();
    expect(within(rows[0]).getByText("ok")).toBeInTheDocument();
    expect(within(rows[1]).getByText("denied")).toBeInTheDocument();
    expect(within(rows[1]).getByText("203.0.113.9")).toBeInTheDocument();
  });

  it("shows an entry's request id, instance, revision and structured detail on demand", async () => {
    renderAudit({});
    const user = userEvent.setup();
    expect(screen.queryByText("abc-1")).not.toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Details" }));
    expect(screen.getByText("abc-1")).toBeInTheDocument();
    expect(screen.getByText("i-1")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.getByText(/subsystem created: shop/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByText("abc-1")).not.toBeInTheDocument();
  });

  it("says plainly that a refused attempt committed nothing", async () => {
    renderAudit({ "GET /api/audit?limit=50": page([auditEntry({ outcome: "denied", revision: null })]) });
    await userEvent.setup().click(await screen.findByRole("button", { name: "Details" }));
    expect(screen.getByText(/nothing was committed/)).toBeInTheDocument();
  });

  it("explains an empty log, and a filter that matches nothing", async () => {
    const api = renderAudit({
      "GET /api/audit?limit=50": page([]),
      "GET /api/audit?limit=50&actor=ghost": page([]),
    });
    expect(await screen.findByText("Nothing recorded yet")).toBeInTheDocument();
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Who"), "ghost");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    expect(await screen.findByText("Nothing matches")).toBeInTheDocument();
    expect(api.called("GET", "/api/audit?limit=50&actor=ghost")).toHaveLength(1);
  });

  it("sends each filter and can clear them", async () => {
    const api = renderAudit({
      "GET /api/audit?limit=50&actor=ann&action=route.create&outcome=failed": page([auditEntry({ outcome: "failed" })]),
    });
    await screen.findAllByRole("row");
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Who"), " ann ");
    await user.type(screen.getByLabelText("Action"), "route.create");
    await user.selectOptions(screen.getByLabelText("Outcome"), "failed");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(api.called("GET", "/api/audit?limit=50&actor=ann&action=route.create&outcome=failed")).toHaveLength(1),
    );
    await user.click(await screen.findByRole("button", { name: "Clear" }));
    expect(screen.getByLabelText("Who")).toHaveValue("");
  });

  it("asks for a time window as a since timestamp", async () => {
    const api = renderAudit({});
    await screen.findAllByRole("row");
    const user = userEvent.setup();
    await user.selectOptions(screen.getByLabelText("When"), "hour");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(api.calls.some((call) => call.path.includes("since="))).toBe(true));
    const since = Number(new URL(api.calls[api.calls.length - 1].path, "http://x").searchParams.get("since"));
    expect(Date.now() - since).toBeGreaterThan(3_500_000);
    expect(Date.now() - since).toBeLessThan(3_700_000);
  });

  it("loads older entries a page at a time", async () => {
    const api = renderAudit({
      "GET /api/audit?limit=50": page([auditEntry({ id: 9, target: "newer" })], 9),
      "GET /api/audit?limit=50&before=9": page([auditEntry({ id: 4, target: "older" })]),
    });
    const user = userEvent.setup();
    await screen.findByText("newer");
    await user.click(screen.getByRole("button", { name: "Load older entries" }));
    expect(await screen.findByText("older")).toBeInTheDocument();
    expect(screen.getByText("newer")).toBeInTheDocument();
    expect(api.called("GET", "/api/audit?limit=50&before=9")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Load older entries" })).not.toBeInTheDocument();
  });

  it("shows why the log could not be loaded", async () => {
    renderAudit({ "GET /api/audit?limit=50": apiError(500, "INTERNAL", "broken") });
    expect(await screen.findByText(/could not load the audit log: broken/i)).toBeInTheDocument();
  });

  it("lets an administrator export the log, and says when that fails", async () => {
    renderAudit({});
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Export NDJSON" }));
    expect(downloadAuditExport).toHaveBeenCalledWith(undefined);
    vi.mocked(downloadAuditExport).mockRejectedValueOnce(new Error("The export was refused"));
    await user.click(screen.getByRole("button", { name: "Export NDJSON" }));
    expect(await screen.findByText("The export was refused")).toBeInTheDocument();
  });

  it("lets a viewer read the log but not export it", async () => {
    renderAudit({}, "viewer");
    expect(await screen.findAllByRole("row")).not.toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Export NDJSON" })).not.toBeInTheDocument();
  });
});
