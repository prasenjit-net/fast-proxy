import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { certificate, envelope, statusTls } from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import type { Certificate, StatusTls } from "../lib/types";
import CertificateHealthCard from "./CertificateHealthCard";

function renderCard(tls: Partial<StatusTls>, certificates: Certificate[] = []) {
  mockApi({
    "GET /api/status": {
      body: envelope({
        tls: statusTls(tls),
        listeners: [],
        database: "",
        subsystems: { total: 0, enabled: 0 },
        upstreams: { total: 0, healthy: 0, unhealthy: [] },
        startedAtMs: 0,
        uptimeSecs: 0,
      }),
    },
    "GET /api/certificates": { body: envelope(certificates) },
  });
  renderPage(CertificateHealthCard, { path: "/" });
}

const soonest = (days: number) => ({ name: "shop", notAfterMs: 0, secondsLeft: days * 86_400 });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CertificateHealthCard", () => {
  it("shows the soonest expiry and is healthy well before it", async () => {
    renderCard({ soonestExpiry: soonest(60), certificates: 1 }, [certificate()]);
    expect(await screen.findByText("healthy")).toBeInTheDocument();
    expect(screen.getByText(/Soonest expiry:/)).toHaveTextContent("shop in 60 days");
  });

  it("warns under 14 days", async () => {
    renderCard({ soonestExpiry: soonest(5), certificates: 1 }, [certificate()]);
    expect(await screen.findByText("expiring soon")).toBeInTheDocument();
  });

  it("raises an expired certificate", async () => {
    renderCard({ soonestExpiry: soonest(-2), certificates: 1 }, [certificate()]);
    expect(await screen.findByText("expired")).toBeInTheDocument();
    expect(screen.getByText(/Expired:/)).toHaveTextContent("shop expired 2 days ago");
  });

  it("explains that TLS is off", async () => {
    renderCard({ enabled: false, listen: null });
    expect(await screen.findByText("TLS off")).toBeInTheDocument();
    expect(screen.getByText(/no HTTPS listener runs/)).toBeInTheDocument();
    expect(screen.getByText("No certificates yet.")).toBeInTheDocument();
  });

  it("warns that only development certificates exist", async () => {
    renderCard({ soonestExpiry: soonest(300), certificates: 1 }, [certificate({ source: "self_signed" })]);
    expect(await screen.findByText(/Only development \(self-signed\) certificates exist/)).toBeInTheDocument();
  });

  it("raises a key mismatch and unreadable certificates", async () => {
    renderCard({ vault: "key mismatch", unreadable: 2, certificates: 2 });
    expect(await screen.findByText("needs attention")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(/master key does not match/);
  });

  it("counts unreadable certificates when the vault itself is fine", async () => {
    renderCard({ unreadable: 1, certificates: 1 });
    expect(await screen.findByRole("alert")).toHaveTextContent("1 certificate cannot be read.");
  });

  it("says the status is unavailable when it cannot be fetched", async () => {
    mockApi({});
    renderPage(CertificateHealthCard, { path: "/" });
    expect(await screen.findByText("Certificate status is unavailable.")).toBeInTheDocument();
  });
});
