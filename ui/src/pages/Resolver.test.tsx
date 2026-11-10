import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Resolution } from "../lib/types";
import { envelope } from "../test/fixtures";
import { apiError, mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import ResolverPage from "./Resolver";

const matched: Resolution = {
  matched: true,
  reason: null,
  subsystem: { id: 1, name: "shop" },
  route: {
    id: 7,
    prefix: "/api",
    upstreamName: "backend",
    algorithm: "round_robin",
    endpoints: [
      {
        id: 1,
        addr: "127.0.0.1:3000",
        weight: 1,
        state: "enabled",
        eligible: true,
        upstreamUrl: "http://127.0.0.1:3000/v2/users?page=2",
      },
      {
        id: 2,
        addr: "127.0.0.1:3001",
        weight: 1,
        state: "draining",
        eligible: false,
        upstreamUrl: "http://127.0.0.1:3001/v2/users?page=2",
      },
    ],
    stripPrefix: true,
    upstreamPrefix: "/v2",
    preserveHost: false,
    upstreamScheme: "http",
  },
  https: { servedOverHttps: false, plainHttpRedirect: null },
};

const unmatched = (reason: NonNullable<Resolution["reason"]>, withSubsystem = false): Resolution => ({
  matched: false,
  reason,
  subsystem: withSubsystem ? { id: 1, name: "shop" } : null,
  route: null,
  https: null,
});

function renderResolver() {
  return renderPage(ResolverPage, { path: "/resolver" });
}

async function resolve(host: string, path = "/") {
  await userEvent.type(screen.getByLabelText("Host"), host);
  await userEvent.clear(screen.getByLabelText("Path"));
  await userEvent.type(screen.getByLabelText("Path"), path);
  await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ResolverPage", () => {
  it("shows the subsystem, route, and the exact upstream URL for a match", async () => {
    const api = mockApi({ "POST /api/resolve": { body: envelope(matched, 6) } });
    renderResolver();
    await screen.findByLabelText("Host");

    await resolve("shop.example.com", "/api/users?page=2");

    expect(await screen.findByText("matched")).toBeInTheDocument();
    expect(screen.getByText("http://127.0.0.1:3000/v2/users?page=2")).toBeInTheDocument();
    expect(screen.getByText("round_robin")).toBeInTheDocument();
    expect(screen.getByText("Not eligible")).toBeInTheDocument();
    expect(screen.getByText("(draining)")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "shop" })).toHaveAttribute("href", "/subsystems/1");
    expect(screen.getByText("/api")).toBeInTheDocument();
    expect(screen.getByText("strip prefix, prepend /v2, rewrite Host header")).toBeInTheDocument();
    expect(screen.getByText("against config version 6")).toBeInTheDocument();
    expect(api.called("POST", "/api/resolve")[0].body).toEqual({
      host: "shop.example.com",
      path: "/api/users?page=2",
    });
  });

  it("shows how HTTPS applies: service, the plain-HTTP redirect and the scheme to the upstream", async () => {
    const withHttps: Resolution = {
      ...matched,
      route: matched.route && { ...matched.route, upstreamScheme: "https" },
      https: {
        servedOverHttps: true,
        plainHttpRedirect: { status: 308, location: "https://shop.example.com/api/users?page=2" },
      },
    };
    mockApi({ "POST /api/resolve": { body: envelope(withHttps, 6) } });
    renderResolver();
    await screen.findByLabelText("Host");
    await resolve("shop.example.com", "/api/users?page=2");

    expect(await screen.findByText("served over HTTPS")).toBeInTheDocument();
    expect(screen.getByText(/redirected with 308 to/)).toBeInTheDocument();
    expect(screen.getByText("https://shop.example.com/api/users?page=2")).toBeInTheDocument();
    expect(screen.getByText("https", { selector: "code" })).toBeInTheDocument();
  });

  it("says a host is not served over HTTPS and has no redirect when that is so", async () => {
    mockApi({ "POST /api/resolve": { body: envelope(matched, 6) } });
    renderResolver();
    await screen.findByLabelText("Host");
    await resolve("shop.example.com");
    expect(await screen.findByText(/not served over HTTPS/)).toBeInTheDocument();
    expect(screen.getByText("served as usual (no redirect)")).toBeInTheDocument();
  });

  it.each([
    ["unknown_host", false, /no subsystem answers to this host.*404/i],
    ["subsystem_disabled", true, /subsystem is disabled.*503/i],
    ["no_route", true, /no route matching this path.*404/i],
  ] as const)("explains why nothing matched: %s", async (reason, withSubsystem, explanation) => {
    mockApi({ "POST /api/resolve": { body: envelope(unmatched(reason, withSubsystem)) } });
    renderResolver();
    await screen.findByLabelText("Host");

    await resolve("nope.example.com");

    expect(await screen.findByText("no match")).toBeInTheDocument();
    expect(screen.getByText(explanation)).toBeInTheDocument();
    expect(screen.queryByText("Forwarded to")).not.toBeInTheDocument();
    if (withSubsystem) {
      expect(screen.getByRole("link", { name: "shop" })).toBeInTheDocument();
    } else {
      expect(screen.queryByText("Subsystem")).not.toBeInTheDocument();
    }
  });

  it("checks the inputs before asking the server", async () => {
    const api = mockApi({});
    renderResolver();
    await screen.findByLabelText("Host");

    await userEvent.clear(screen.getByLabelText("Path"));
    await userEvent.type(screen.getByLabelText("Path"), "no-slash");
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));

    const alerts = await screen.findAllByRole("alert");
    expect(alerts.map((alert) => alert.textContent)).toEqual([
      "Enter the host name a client would send",
      "Must start with '/'",
    ]);
    expect(api.called("POST", "/api/resolve")).toHaveLength(0);
  });

  it("shows a field error from the server next to its input", async () => {
    mockApi({
      "POST /api/resolve": apiError(400, "VALIDATION_FAILED", "validation failed", [
        { field: "path", message: "is not a valid path" },
      ]),
    });
    renderResolver();
    await screen.findByLabelText("Host");
    await resolve("shop.example.com", "/a b");

    expect(await screen.findByText("is not a valid path")).toBeInTheDocument();
    expect(screen.getByLabelText("Path")).toHaveAttribute("aria-invalid", "true");
    expect(screen.queryByText("Result")).not.toBeInTheDocument();
  });

  it("replaces an old result with a new one", async () => {
    let calls = 0;
    mockApi({
      "POST /api/resolve": () => ({
        body: envelope(++calls === 1 ? matched : unmatched("unknown_host")),
      }),
    });
    renderResolver();
    await screen.findByLabelText("Host");
    await userEvent.type(screen.getByLabelText("Host"), "shop.example.com");
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    expect(await screen.findByText("matched")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    expect(await screen.findByText("no match")).toBeInTheDocument();
    expect(screen.queryByText("matched")).not.toBeInTheDocument();
  });
});
