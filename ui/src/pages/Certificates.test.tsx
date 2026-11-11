import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { adminStatus, caBundle, certificate, envelope, subsystemSummary, tlsStatus } from "../test/fixtures";
import { apiError, mockApi, type Reply } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import type { Certificate } from "../lib/types";
import CertificatesPage from "./Certificates";

const cert = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
const key = "-----BEGIN PRIVATE KEY-----\nBBBB\n-----END PRIVATE KEY-----";

function renderCertificates(options: {
  certificates?: Certificate[];
  tls?: Parameters<typeof tlsStatus>[0];
  handlers?: Record<string, Reply>;
  bundles?: ReturnType<typeof caBundle>[];
}) {
  const api = mockApi({
    "GET /api/certificates": { body: envelope(options.certificates ?? []) },
    "GET /api/tls": { body: envelope(tlsStatus(options.tls)) },
    "GET /api/subsystems": { body: envelope([subsystemSummary({ id: 1, name: "shop" })]) },
    "GET /api/ca-bundles": { body: envelope(options.bundles ?? []) },
    ...options.handlers,
  });
  renderPage(CertificatesPage, { path: "/certificates", entry: "/certificates" });
  return api;
}

const dialog = () => within(screen.getByRole("dialog"));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CertificatesPage", () => {
  it("lists certificates with source, expiry, state and the subsystems using them", async () => {
    renderCertificates({
      certificates: [
        certificate({ id: 3, name: "shop", usedBy: [1] }),
        certificate({ id: 4, name: "old", source: "self_signed", state: "expired", secondsLeft: -3 * 86_400, usedBy: [] }),
      ],
    });
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(within(rows[0]).getByText("shop", { selector: "div" })).toBeInTheDocument();
    expect(within(rows[0]).getByText("uploaded")).toBeInTheDocument();
    expect(within(rows[0]).getByText("in 90 days")).toBeInTheDocument();
    expect(within(rows[0]).getByText("ok")).toBeInTheDocument();
    expect(within(rows[0]).getByText("shop", { selector: "td" })).toBeInTheDocument();
    expect(within(rows[1]).getByText("self-signed")).toBeInTheDocument();
    expect(within(rows[1]).getByText("expired 3 days ago")).toBeInTheDocument();
    expect(within(rows[1]).getByText("unused")).toBeInTheDocument();
  });

  it("guides an empty list", async () => {
    renderCertificates({});
    expect(await screen.findByText("No certificates yet")).toBeInTheDocument();
  });

  it("explains when TLS is off", async () => {
    renderCertificates({ tls: { enabled: false, listen: null } });
    expect(await screen.findByText(/no HTTPS listener runs/i)).toBeInTheDocument();
  });

  it("shows the listener and policy when TLS is on", async () => {
    renderCertificates({ tls: { enabled: true, listen: "0.0.0.0:8443", minVersion: "1.3", http2: false } });
    expect(await screen.findByText("0.0.0.0:8443")).toBeInTheDocument();
    expect(screen.getByText("TLS 1.3, HTTP/2 off")).toBeInTheDocument();
  });

  it("raises a master key mismatch as an alert", async () => {
    renderCertificates({ tls: { vault: "key mismatch" } });
    expect(await screen.findByRole("alert")).toHaveTextContent(/master key does not match/i);
  });

  it("warns while only development certificates exist", async () => {
    renderCertificates({ certificates: [certificate({ source: "self_signed", name: "dev-shop" })] });
    expect(await screen.findByText(/Only development \(self-signed\) certificates exist/)).toBeInTheDocument();
  });

  it("does not warn once a real certificate exists", async () => {
    renderCertificates({
      certificates: [certificate({ source: "self_signed", name: "dev" }), certificate({ id: 9, name: "real" })],
    });
    await screen.findByRole("table");
    expect(screen.queryByText(/Only development/)).not.toBeInTheDocument();
  });

  it("uploads a certificate, sends the key once and reports a warning from the server", async () => {
    const api = renderCertificates({
      handlers: {
        "POST /api/certificates": {
          status: 201,
          body: envelope(certificate({ name: "fresh", warnings: ["the certificate expires in under 14 days"] })),
        },
      },
    });
    await userEvent.click(await screen.findByRole("button", { name: /upload a certificate/i }));
    await userEvent.type(dialog().getByLabelText("Name"), "fresh");
    await userEvent.click(dialog().getByLabelText("Certificate chain (PEM)"));
    await userEvent.paste(cert);
    await userEvent.click(dialog().getByLabelText("Private key (PEM)"));
    await userEvent.paste(key);
    await userEvent.click(dialog().getByRole("button", { name: "Upload" }));
    expect(await screen.findByText(/Uploaded certificate fresh \(the certificate expires in under 14 days\)/)).toBeInTheDocument();
    expect(api.called("POST", "/api/certificates")[0].body).toEqual({ name: "fresh", certPem: cert, keyPem: key });
  });

  it("generates a self-signed certificate for the host names given", async () => {
    const api = renderCertificates({
      handlers: { "POST /api/certificates/self-signed": { status: 201, body: envelope(certificate()) } },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Generate self-signed" }));
    await userEvent.click(dialog().getByRole("button", { name: "Generate" }));
    expect(await dialog().findAllByRole("alert")).toHaveLength(2);
    await userEvent.type(dialog().getByLabelText("Name"), "scratch");
    await userEvent.type(dialog().getByLabelText("Host names"), "a.example.com, *.b.example.com");
    await userEvent.click(dialog().getByRole("button", { name: "Generate" }));
    expect(await screen.findByText("Generated certificate scratch")).toBeInTheDocument();
    expect(api.called("POST", "/api/certificates/self-signed")[0].body).toEqual({
      name: "scratch",
      names: ["a.example.com", "*.b.example.com"],
    });
  });

  it("renames a certificate", async () => {
    const api = renderCertificates({
      certificates: [certificate({ id: 3, name: "shop" })],
      handlers: { "PUT /api/certificates/3": { body: envelope(certificate({ name: "storefront" })) } },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Rename shop" }));
    await userEvent.clear(dialog().getByLabelText("Name"));
    await userEvent.type(dialog().getByLabelText("Name"), "storefront");
    await userEvent.click(dialog().getByRole("button", { name: "Rename" }));
    expect(await screen.findByText("Renamed to storefront")).toBeInTheDocument();
    expect(api.called("PUT", "/api/certificates/3")[0].body).toEqual({ name: "storefront" });
  });

  it("replaces only what was pasted: a renewal with a new chain and key leaves the name alone", async () => {
    const api = renderCertificates({
      certificates: [certificate({ id: 3, name: "shop" })],
      handlers: { "PUT /api/certificates/3": { body: envelope(certificate()) } },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Replace" }));
    await userEvent.click(dialog().getByLabelText("Certificate chain (PEM)"));
    await userEvent.paste(cert);
    await userEvent.click(dialog().getByRole("button", { name: "Replace" }));
    expect(await screen.findByText("Replaced certificate shop")).toBeInTheDocument();
    expect(api.called("PUT", "/api/certificates/3")[0].body).toEqual({ certPem: cert });
  });

  it("deletes after confirming, and shows the server's 409 when a subsystem still uses it", async () => {
    const api = renderCertificates({
      certificates: [certificate({ id: 3, name: "shop", usedBy: [1] })],
      handlers: {
        "DELETE /api/certificates/3": apiError(409, "CONFLICT", "certificate 3 is used by subsystem(s): shop"),
      },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Delete shop" }));
    expect(dialog().getByText(/and its private key/)).toBeInTheDocument();
    await userEvent.click(dialog().getByRole("button", { name: "Delete certificate" }));
    expect(await screen.findByText(/used by subsystem\(s\): shop/)).toBeInTheDocument();
    expect(api.called("DELETE", "/api/certificates/3")).toHaveLength(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("deletes an unused certificate", async () => {
    renderCertificates({
      certificates: [certificate({ id: 3, name: "shop" })],
      handlers: { "DELETE /api/certificates/3": { body: { version: 9 } } },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Delete shop" }));
    await userEvent.click(dialog().getByRole("button", { name: "Delete certificate" }));
    expect(await screen.findByText("Deleted certificate shop")).toBeInTheDocument();
  });

  describe("CA bundles", () => {
    it("lists bundles with their certificates", async () => {
      renderCertificates({ bundles: [caBundle({ name: "corp" })] });
      expect(await screen.findByText("corp")).toBeInTheDocument();
      expect(screen.getByText(/1 certificate: CN=Corp Root/)).toBeInTheDocument();
    });

    it("uploads a bundle", async () => {
      const api = renderCertificates({
        handlers: { "POST /api/ca-bundles": { status: 201, body: envelope(caBundle()) } },
      });
      await userEvent.click(await screen.findByRole("button", { name: /upload a bundle/i }));
      await userEvent.click(dialog().getByRole("button", { name: "Upload bundle" }));
      expect(await dialog().findAllByRole("alert")).toHaveLength(2);
      await userEvent.type(dialog().getByLabelText("Name"), "corp");
      await userEvent.click(dialog().getByLabelText("CA certificates (PEM)"));
      await userEvent.paste(cert);
      await userEvent.click(dialog().getByRole("button", { name: "Upload bundle" }));
      expect(await screen.findByText("Uploaded CA bundle corp")).toBeInTheDocument();
      expect(api.called("POST", "/api/ca-bundles")[0].body).toEqual({ name: "corp", pem: cert });
    });

    it("shows the 409 when an upstream still names the bundle", async () => {
      renderCertificates({
        bundles: [caBundle({ id: 2, name: "corp" })],
        handlers: { "DELETE /api/ca-bundles/2": apiError(409, "CONFLICT", "CA bundle corp is used by 1 upstream group(s)") },
      });
      await userEvent.click(await screen.findByRole("button", { name: "Delete corp" }));
      await userEvent.click(dialog().getByRole("button", { name: "Delete bundle" }));
      expect(await screen.findByText(/used by 1 upstream group/)).toBeInTheDocument();
    });

    it("deletes an unused bundle", async () => {
      const api = renderCertificates({
        bundles: [caBundle({ id: 2, name: "corp" })],
        handlers: { "DELETE /api/ca-bundles/2": { body: { version: 5 } } },
      });
      await userEvent.click(await screen.findByRole("button", { name: "Delete corp" }));
      await userEvent.click(dialog().getByRole("button", { name: "Delete bundle" }));
      await waitFor(() => expect(api.called("DELETE", "/api/ca-bundles/2")).toHaveLength(1));
      expect(await screen.findByText("Deleted CA bundle corp")).toBeInTheDocument();
    });
  });

  describe("the admin plane's own certificate", () => {
    const adminCert = certificate({ id: 8, name: "admin", source: "self_signed", usedBy: [] });

    it("says which certificate this page is served with and warns that a generated one makes browsers warn", async () => {
      renderCertificates({
        certificates: [adminCert],
        handlers: { "GET /api/status": { body: envelope({ admin: adminStatus({ tlsCertificate: "admin" }) }) } },
      });
      expect(await screen.findByText("Admin plane")).toBeInTheDocument();
      expect(screen.getByText(/served over HTTPS with the certificate/i)).toBeInTheDocument();
      expect(screen.getByText(/generated self-signed certificate, so browsers warn/i)).toBeInTheDocument();
    });

    it("marks it in the list, may be replaced, and cannot be renamed or deleted from here", async () => {
      renderCertificates({
        certificates: [adminCert, certificate({ id: 9, name: "other", usedBy: [] })],
        handlers: { "GET /api/status": { body: envelope({ admin: adminStatus({ tlsCertificate: "admin" }) }) } },
      });
      await screen.findByText("Admin plane");
      const rows = within(screen.getByRole("table")).getAllByRole("row").slice(1);
      expect(within(rows[0]).getByText("admin plane")).toBeInTheDocument();
      expect(within(rows[0]).getByRole("button", { name: "Replace" })).toBeInTheDocument();
      expect(within(rows[0]).queryByRole("button", { name: /rename admin/i })).not.toBeInTheDocument();
      expect(within(rows[0]).queryByRole("button", { name: /delete admin/i })).not.toBeInTheDocument();
      expect(within(rows[1]).getByRole("button", { name: /rename other/i })).toBeInTheDocument();
      expect(within(rows[1]).getByRole("button", { name: /delete other/i })).toBeInTheDocument();
    });

    it("does not warn about browsers when the admin certificate was uploaded", async () => {
      renderCertificates({
        certificates: [certificate({ id: 8, name: "admin", source: "uploaded" })],
        handlers: { "GET /api/status": { body: envelope({ admin: adminStatus({ tlsCertificate: "admin" }) }) } },
      });
      await screen.findByText("Admin plane");
      expect(screen.queryByText(/browsers warn about this page/i)).not.toBeInTheDocument();
    });

    it("explains that the admin plane is plain HTTP when it does not serve TLS", async () => {
      renderCertificates({
        handlers: { "GET /api/status": { body: envelope({ admin: adminStatus({ tlsCertificate: null }) }) } },
      });
      expect(await screen.findByText(/served over plain HTTP/i)).toBeInTheDocument();
    });
  });
});
