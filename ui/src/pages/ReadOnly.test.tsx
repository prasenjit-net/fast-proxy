// A viewer's UI is read-only: every control that would change something is
// hidden, not merely refused by the server. Each page is rendered twice, as an
// administrator (to prove the controls exist) and as a viewer (to prove none do).
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  adminStatus,
  caBundle,
  certificate,
  envelope,
  httpsSettings,
  revisionPage,
  subsystemDetail,
  subsystemSummary,
  tlsStatus,
} from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import CertificatesPage from "./Certificates";
import RevisionsPage from "./Revisions";
import SubsystemDetailPage from "./SubsystemDetail";
import SubsystemsPage from "./Subsystems";

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Every control a person could press or type into. */
const controls = () => [
  ...screen.queryAllByRole("button"),
  ...screen.queryAllByRole("checkbox"),
  ...screen.queryAllByRole("textbox"),
  ...screen.queryAllByRole("combobox"),
];

describe("a viewer's UI is read-only", () => {
  it("the subsystem list has no create button and no on/off switches", async () => {
    mockApi({ "GET /api/subsystems": { body: envelope([subsystemSummary()]) } });
    renderPage(SubsystemsPage, { path: "/subsystems", role: "viewer" });
    expect(await screen.findByText("shop")).toBeInTheDocument();
    expect(controls().map((c) => c.textContent)).toEqual([]);
    expect(screen.getByText("On")).toBeInTheDocument();
  });

  it("an administrator has them", async () => {
    mockApi({ "GET /api/subsystems": { body: envelope([subsystemSummary()]) } });
    renderPage(SubsystemsPage, { path: "/subsystems", role: "admin" });
    expect(await screen.findByRole("button", { name: /new subsystem/i })).toBeInTheDocument();
    expect(screen.getByRole("checkbox")).toBeInTheDocument();
  });

  const detail = () =>
    subsystemDetail({
      https: httpsSettings({
        enabled: true,
        certificateId: 3,
        effective: true,
        certificate: { name: "shop", source: "uploaded", notAfterMs: Date.now() + 86_400_000, uncoveredHosts: [] },
      }),
    });

  it("a subsystem page shows everything and offers no control at all", async () => {
    mockApi({
      "GET /api/subsystems/1": { body: envelope(detail()) },
      "GET /api/tls": { body: envelope(tlsStatus()) },
      "GET /api/maintenance": { body: envelope([]) },
    });
    renderPage(SubsystemDetailPage, { path: "/subsystems/$id", entry: "/subsystems/1", role: "viewer" });
    expect(await screen.findByRole("heading", { name: "shop", level: 2 })).toBeInTheDocument();
    // What there is to read is all there ...
    expect(screen.getByText("shop.example.com")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:3000")).toBeInTheDocument();
    expect(screen.getByText("/api")).toBeInTheDocument();
    expect(screen.getByText("The shop")).toBeInTheDocument();
    // ... including that nothing is in maintenance, and nothing can be pressed or typed into.
    await waitFor(() => expect(screen.getAllByText("open").length).toBeGreaterThan(0));
    expect(controls().map((c) => c.getAttribute("aria-label") ?? c.textContent)).toEqual([]);
  });

  it("the same page has all of them for an administrator", async () => {
    mockApi({
      "GET /api/subsystems/1": { body: envelope(detail()) },
      "GET /api/tls": { body: envelope(tlsStatus()) },
      "GET /api/maintenance": { body: envelope([]) },
    });
    renderPage(SubsystemDetailPage, { path: "/subsystems/$id", entry: "/subsystems/1", role: "admin" });
    await screen.findByRole("heading", { name: "shop", level: 2 });
    for (const name of [
      /delete subsystem/i,
      /save settings/i,
      /add host/i,
      /add upstream/i,
      /add endpoint/i,
      /add route/i,
      /edit upstream/i,
      /drain endpoint/i,
      /edit route/i,
      /turn on maintenance for subsystem shop/i,
      /turn on maintenance for route/i,
      /upload a certificate/i,
    ]) {
      expect(screen.getAllByRole("button", { name }).length, String(name)).toBeGreaterThan(0);
    }
  });

  it("the certificates page offers no upload, replace, rename or delete, and says what serves the admin plane", async () => {
    mockApi({
      "GET /api/certificates": { body: envelope([certificate({ id: 3, name: "admin", source: "self_signed" })]) },
      "GET /api/tls": { body: envelope(tlsStatus()) },
      "GET /api/subsystems": { body: envelope([subsystemSummary()]) },
      "GET /api/ca-bundles": { body: envelope([caBundle()]) },
      "GET /api/status": { body: envelope({ admin: adminStatus({ tlsCertificate: "admin" }) }) },
    });
    renderPage(CertificatesPage, { path: "/certificates", role: "viewer" });
    expect(await screen.findByText("corp")).toBeInTheDocument();
    expect(controls().map((c) => c.getAttribute("aria-label") ?? c.textContent)).toEqual([]);
    expect(screen.getByText("Admin plane")).toBeInTheDocument();
  });

  it("the revisions page lets a viewer look at the history but not roll back", async () => {
    mockApi({ "GET /api/revisions?limit=25": { body: revisionPage() } });
    renderPage(RevisionsPage, { path: "/revisions", role: "viewer" });
    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(within(rows[1]).getByRole("button", { name: "View" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /roll back/i })).not.toBeInTheDocument();
  });
});
