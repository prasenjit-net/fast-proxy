import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api as clientApi } from "../lib/api";
import { clearToken, getToken, setToken } from "../lib/session";
import { envelope, whoami } from "../test/fixtures";
import { apiError, mockApi } from "../test/mockApi";
import { SessionProvider, useSession } from "./SessionContext";

function Who() {
  const { who, canWrite, isAdmin, signOut } = useSession();
  return (
    <div>
      <span data-testid="who">
        {who.name}:{who.role}:{String(canWrite)}:{String(isAdmin)}
      </span>
      <button onClick={signOut}>sign out</button>
    </div>
  );
}

function renderSession() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <SessionProvider>
        <Who />
      </SessionProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  clearToken();
  vi.unstubAllGlobals();
});

describe("SessionProvider", () => {
  it("goes straight in when the server asks for no token (authentication off)", async () => {
    mockApi({ "GET /api/whoami": { body: envelope(whoami({ name: "anonymous", auth: "off" })) } });
    renderSession();
    expect(await screen.findByTestId("who")).toHaveTextContent("anonymous:admin:true:true");
  });

  it("shows the sign-in page when the server wants a token", async () => {
    mockApi({ "GET /api/whoami": apiError(401, "UNAUTHORIZED", "a valid admin token is required") });
    renderSession();
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(screen.queryByTestId("who")).not.toBeInTheDocument();
  });

  it("rejects a wrong token with a message and stays on the sign-in page", async () => {
    const api = mockApi({ "GET /api/whoami": apiError(401, "UNAUTHORIZED", "no") });
    renderSession();
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Admin token"), "fp_1.wrong");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("That token was not accepted");
    expect(getToken()).toBeNull();
    expect(api.called("GET", "/api/whoami")).toHaveLength(2);
  });

  it("asks for a token before it sends anything", async () => {
    mockApi({ "GET /api/whoami": apiError(401, "UNAUTHORIZED", "no") });
    renderSession();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Sign in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Paste an admin token");
  });

  it("signs in with a good token, keeps it for the tab and sends it from then on", async () => {
    const api = mockApi({
      "GET /api/whoami": (call) =>
        call.headers.Authorization === "Bearer fp_2.right"
          ? { body: envelope(whoami({ name: "ann", role: "viewer" })) }
          : apiError(401, "UNAUTHORIZED", "no"),
      "GET /api/status": (call) => ({ body: { sent: call.headers.Authorization } }),
    });
    renderSession();
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Admin token"), "  fp_2.right  ");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByTestId("who")).toHaveTextContent("ann:viewer:false:false");
    expect(getToken()).toBe("fp_2.right");
    expect(window.sessionStorage.getItem("fast-proxy-token")).toBe("fp_2.right");
    await expect(clientApi.status()).resolves.toEqual({ sent: "Bearer fp_2.right" });
    expect(api.called("GET", "/api/whoami")).toHaveLength(2);
  });

  it("restores a token kept in the tab and signs in without asking", async () => {
    setToken("fp_1.kept");
    mockApi({ "GET /api/whoami": { body: envelope(whoami({ name: "kept" })) } });
    renderSession();
    expect(await screen.findByTestId("who")).toHaveTextContent("kept:admin:true:true");
  });

  it("signing out clears the token and shows the sign-in page", async () => {
    setToken("fp_1.kept");
    mockApi({ "GET /api/whoami": { body: envelope(whoami()) } });
    renderSession();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "sign out" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(getToken()).toBeNull();
    expect(window.sessionStorage.getItem("fast-proxy-token")).toBeNull();
  });

  it("sends the user back to sign in, with a reason, when the token stops being accepted", async () => {
    setToken("fp_1.kept");
    mockApi({
      "GET /api/whoami": { body: envelope(whoami()) },
      "GET /api/status": apiError(401, "UNAUTHORIZED", "revoked"),
    });
    renderSession();
    await screen.findByTestId("who");
    await act(async () => {
      await clientApi.status().catch(() => undefined);
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Your session ended");
    expect(screen.queryByTestId("who")).not.toBeInTheDocument();
    expect(getToken()).toBeNull();
  });

  it("offers a retry when the server cannot be reached", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("offline")));
    renderSession();
    expect(await screen.findByRole("heading", { name: "Cannot reach the server" })).toBeInTheDocument();
    mockApi({ "GET /api/whoami": { body: envelope(whoami()) } });
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(screen.getByTestId("who")).toBeInTheDocument());
  });
});
