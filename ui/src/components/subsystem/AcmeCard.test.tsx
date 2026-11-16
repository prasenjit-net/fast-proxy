import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, httpsSettings, subsystemDetail, tlsStatus } from "../../test/fixtures";
import { apiError, mockApi, type Reply } from "../../test/mockApi";
import { renderPage } from "../../test/renderPage";
import type { AcmeOverview, AcmeStatus, HttpsSettings, SubsystemAcme } from "../../lib/types";
import HttpsSection from "./HttpsSection";

const HOUR = 3_600_000;

function status(overrides: Partial<AcmeStatus> = {}): AcmeStatus {
  return {
    subsystemId: 1,
    subsystem: "shop",
    state: "valid",
    eligibleNames: ["shop.example.com"],
    skippedNames: [],
    certificateId: 7,
    certificateName: "acme-shop",
    notAfterMs: Date.now() + 80 * 24 * HOUR,
    nextAttemptAtMs: null,
    failures: 0,
    lastError: "",
    lastAttemptAtMs: Date.now() - HOUR,
    lastSuccessAtMs: Date.now() - HOUR,
    instance: "42-abc",
    ...overrides,
  };
}

function view(overrides: Partial<SubsystemAcme> = {}): SubsystemAcme {
  return {
    available: true,
    acmeEnabled: false,
    status: null,
    preview: {
      eligibleNames: ["shop.example.com"],
      skippedNames: [{ name: "*.cdn.example.com", reason: "wildcard_needs_dns01" }],
    },
    ...overrides,
  };
}

function overview(staging = false): AcmeOverview {
  return {
    enabled: true,
    directory: "https://acme.example/dir",
    staging,
    allowedDomains: ["*.example.com"],
    accountRegistered: true,
    lease: null,
    subsystems: [],
  };
}

function render(
  https: Partial<HttpsSettings>,
  acme: SubsystemAcme,
  handlers: Record<string, Reply> = {},
  { role = "admin" as "admin" | "viewer", staging = false } = {},
) {
  const api = mockApi({
    "GET /api/tls": { body: envelope(tlsStatus({ enabled: true })) },
    "GET /api/subsystems/1/acme": { body: envelope(acme) },
    "GET /api/acme": { body: envelope(overview(staging)) },
    ...handlers,
  });
  const detail = subsystemDetail({
    https: httpsSettings({ enabled: true, effective: true, ...https }),
  });
  renderPage(() => <HttpsSection subsystem={detail} />, { path: "/x", entry: "/x", role });
  return api;
}

const card = async () => within(await screen.findByLabelText("ACME"));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("AcmeCard", () => {
  it("explains how to turn ACME on when the proxy has it off, with no switch", async () => {
    render({}, view({ available: false, preview: null }));
    const acme = await card();
    expect(acme.getByRole("note")).toHaveTextContent(/ACME is off for this proxy/);
    expect(acme.queryByLabelText(/automatically/)).not.toBeInTheDocument();
  });

  it("previews what an order would cover, and turning it on sends the switch with the rest", async () => {
    const api = render({}, view(), {
      "PUT /api/subsystems/1": { body: envelope(subsystemDetail(), 6) },
    });
    const acme = await card();
    expect(acme.getByText(/An order would cover/)).toBeInTheDocument();
    expect(acme.getByText("shop.example.com")).toBeInTheDocument();
    expect(acme.getByText(/wildcards need DNS-01/)).toBeInTheDocument();
    await userEvent.click(acme.getByLabelText("Get and renew this subsystem's certificate automatically"));
    await waitFor(() => expect(api.called("PUT", "/api/subsystems/1")).toHaveLength(1));
    const sent = api.called("PUT", "/api/subsystems/1")[0].body as { https: { acmeEnabled: boolean } };
    expect(sent.https.acmeEnabled).toBe(true);
    expect(await screen.findByText(/ACME on: the certificate will be ordered shortly/)).toBeInTheDocument();
  });

  it("shows a failed order with its error and next attempt, and renews on request", async () => {
    const api = render(
      { acmeEnabled: true },
      view({
        acmeEnabled: true,
        status: status({
          state: "failed",
          lastError: "unauthorized: the key authorization was not served",
          nextAttemptAtMs: Date.now() + 2 * HOUR,
          skippedNames: [{ name: "shop.other.net", reason: "not_allowed" }],
        }),
      }),
      { "POST /api/subsystems/1/acme/renew": { status: 202 } },
    );
    const acme = await card();
    expect(acme.getByText("failed")).toBeInTheDocument();
    expect(acme.getByText(/key authorization was not served/)).toBeInTheDocument();
    expect(acme.getByText(/outside allowed_domains/)).toBeInTheDocument();
    expect(acme.getByText(/in 1h 5\dm|in 2h 0m/)).toBeInTheDocument();
    await userEvent.click(acme.getByRole("button", { name: "Renew now" }));
    expect(await screen.findByText(/Renewal requested/)).toBeInTheDocument();
    expect(api.called("POST", "/api/subsystems/1/acme/renew")).toHaveLength(1);
  });

  it("says why a renewal is refused", async () => {
    render(
      { acmeEnabled: true },
      view({ acmeEnabled: true, status: status() }),
      {
        "POST /api/subsystems/1/acme/renew": apiError(
          429,
          "RATE_LIMITED",
          "too many failed orders in the last hour; wait before forcing another",
        ),
      },
    );
    const acme = await card();
    expect(acme.getByText("certificate issued")).toBeInTheDocument();
    expect(acme.getByText(/acme-shop, expires in 7\dd|acme-shop, expires in 80d/)).toBeInTheDocument();
    await userEvent.click(acme.getByRole("button", { name: "Renew now" }));
    expect(await screen.findByText(/too many failed orders/)).toBeInTheDocument();
  });

  it("marks a staging CA and keeps the attached certificate's manual actions away", async () => {
    render(
      {
        acmeEnabled: true,
        certificateId: 7,
        certificate: { name: "dev-shop", source: "self_signed", notAfterMs: Date.now() + 300 * 24 * HOUR, uncoveredHosts: [] },
      },
      view({ acmeEnabled: true, status: status({ state: "pending", certificateId: null, notAfterMs: null }) }),
      {},
      { staging: true },
    );
    expect((await screen.findAllByText("staging CA")).length).toBe(1);
    expect(screen.getByText(/not trusted by browsers/)).toBeInTheDocument();
    expect(screen.getByText(/Managed by ACME/)).toBeInTheDocument();
    expect(screen.getByText(/serves until ACME issues the real one/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Upload a certificate" })).not.toBeInTheDocument();
  });

  it("a viewer sees the status but neither the switch nor Renew now", async () => {
    render({ acmeEnabled: true }, view({ acmeEnabled: true, status: status() }), {}, { role: "viewer" });
    const acme = await card();
    expect(acme.getByText("certificate issued")).toBeInTheDocument();
    expect(acme.queryByLabelText(/automatically/)).not.toBeInTheDocument();
    expect(acme.queryByRole("button", { name: "Renew now" })).not.toBeInTheDocument();
  });
});
