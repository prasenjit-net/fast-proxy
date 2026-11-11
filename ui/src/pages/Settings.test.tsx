import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { adminStatus, envelope } from "../test/fixtures";
import { mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import SettingsPage from "./Settings";

// Settings reads the server config, the live socket and the theme; the real
// providers open a WebSocket, which these tests do not need.
vi.mock("../context/ConfigContext", () => ({
  useConfig: () => ({
    ui: { appName: "Test", tagline: "t", defaultTheme: "auto", repoUrl: null },
    version: "0.0.0",
    startedAtMs: 1_700_000_000_000,
  }),
}));
vi.mock("../context/LiveContext", () => ({
  useLive: () => ({ status: "online", traffic: null }),
}));
vi.mock("../context/ThemeContext", () => ({
  useTheme: () => ({ mode: "light", setMode: vi.fn() }),
}));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SettingsPage: admin access", () => {
  it("shows who is signed in and how the admin plane is protected", async () => {
    mockApi({
      "GET /api/status": {
        body: envelope({ admin: adminStatus({ tlsCertificate: "admin", activeTokens: 3 }) }),
      },
    });
    renderPage(SettingsPage, { path: "/settings" });
    expect(await screen.findByText("required: a token for every call")).toBeInTheDocument();
    expect(screen.getByText("test-admin (admin)")).toBeInTheDocument();
    expect(screen.getByText("accepted: the channel is protected (loopback or TLS)")).toBeInTheDocument();
    expect(screen.getByText("admin")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("says plainly when keys are refused and when TLS is off", async () => {
    mockApi({
      "GET /api/status": {
        body: envelope({ admin: adminStatus({ protectedChannel: false, tlsCertificate: null }) }),
      },
    });
    renderPage(SettingsPage, { path: "/settings", role: "viewer" });
    expect(await screen.findByText("refused: this channel is not protected")).toBeInTheDocument();
    expect(screen.getByText("none: plain HTTP")).toBeInTheDocument();
    expect(screen.getByText("test-admin (viewer)")).toBeInTheDocument();
  });

  it("warns that anyone can change things when authentication is off", async () => {
    mockApi({ "GET /api/status": { body: envelope({ admin: adminStatus({ auth: "off" }) }) } });
    renderPage(SettingsPage, { path: "/settings" });
    expect(await screen.findByText("off (loopback only)")).toBeInTheDocument();
    expect(screen.getByRole("note")).toHaveTextContent("anyone who can reach this page");
  });
});
