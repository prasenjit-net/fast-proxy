import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope } from "../../test/fixtures";
import { mockApi } from "../../test/mockApi";
import { renderPage } from "../../test/renderPage";
import type { AcmeOverview, AcmeStatus } from "../../lib/types";
import AcmeOverviewSection from "./AcmeOverviewSection";

const base: Omit<AcmeStatus, "subsystemId" | "subsystem" | "state"> = {
  eligibleNames: ["x.example.com"],
  skippedNames: [],
  certificateId: null,
  certificateName: null,
  notAfterMs: null,
  nextAttemptAtMs: null,
  failures: 0,
  lastError: "",
  lastAttemptAtMs: null,
  lastSuccessAtMs: null,
  instance: "1-a",
};

function render(data: AcmeOverview) {
  mockApi({ "GET /api/acme": { body: envelope(data) } });
  renderPage(() => <AcmeOverviewSection />, { path: "/x", entry: "/x" });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("AcmeOverviewSection", () => {
  it("says ACME is off", async () => {
    render({
      enabled: false,
      directory: null,
      staging: false,
      allowedDomains: [],
      accountRegistered: false,
      lease: null,
      subsystems: [],
    });
    expect(await screen.findByText(/ACME is off for this proxy/)).toBeInTheDocument();
  });

  it("shows the CA, the account, the renewing instance and every status, and filters the failing ones", async () => {
    render({
      enabled: true,
      directory: "https://acme-staging.example/dir",
      staging: true,
      allowedDomains: ["*.example.com"],
      accountRegistered: true,
      lease: { holder: "42-abc", expiresAtMs: Date.now() + 5 * 60_000, heldHere: true },
      subsystems: [
        { ...base, subsystemId: 1, subsystem: "shop", state: "valid", notAfterMs: Date.now() + 86_400_000 * 60 },
        { ...base, subsystemId: 2, subsystem: "blog", state: "failed", lastError: "dns: NXDOMAIN" },
      ],
    });
    const section = within(await screen.findByLabelText("ACME"));
    expect(section.getByText(/staging: not publicly trusted/)).toBeInTheDocument();
    expect(section.getByText("registered")).toBeInTheDocument();
    expect(section.getByText(/42-abc \(this one\), lease expires in [45]m/)).toBeInTheDocument();
    expect(section.getByText("shop")).toBeInTheDocument();
    expect(section.getByText("dns: NXDOMAIN")).toBeInTheDocument();
    await userEvent.click(section.getByLabelText("Only failing"));
    expect(section.queryByText("shop")).not.toBeInTheDocument();
    expect(section.getByText("blog")).toBeInTheDocument();
  });

  it("says when no subsystem uses ACME yet", async () => {
    render({
      enabled: true,
      directory: "https://a/dir",
      staging: false,
      allowedDomains: ["a.example"],
      accountRegistered: false,
      lease: null,
      subsystems: [],
    });
    expect(await screen.findByText("No subsystem uses ACME yet.")).toBeInTheDocument();
    expect(screen.getByText("created on the first order")).toBeInTheDocument();
    expect(screen.getByText("none yet")).toBeInTheDocument();
  });
});
