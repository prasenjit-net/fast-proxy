import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, NOW, token } from "../test/fixtures";
import { apiError, mockApi, type Reply } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import TokensPage from "./Tokens";

const LIST = "GET /api/tokens";
const dialog = () => within(screen.getByRole("dialog"));

function renderTokens(handlers: Record<string, Reply> = {}, tokens = [token()], role: "admin" | "viewer" = "admin") {
  const api = mockApi({ [LIST]: { body: envelope(tokens) }, ...handlers });
  renderPage(TokensPage, { path: "/tokens", role });
  return api;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("TokensPage", () => {
  it("lists tokens with role, state, who made them and when they were last used", async () => {
    renderTokens({}, [
      token({ id: 1, name: "ci", role: "viewer", lastUsedAtMs: Date.now() - 120_000 }),
      token({ id: 2, name: "old", role: "admin", revokedAtMs: NOW }),
      token({ id: 3, name: "short", expiresAtMs: 1 }),
      token({ id: 4, name: "test-admin", role: "admin" }),
    ]);
    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(rows).toHaveLength(4);
    expect(within(rows[0]).getByText("viewer")).toBeInTheDocument();
    expect(within(rows[0]).getByText("active")).toBeInTheDocument();
    expect(within(rows[0]).getByText("2m ago")).toBeInTheDocument();
    expect(within(rows[0]).getByText(/by test-admin/)).toBeInTheDocument();
    expect(within(rows[1]).getByText("revoked")).toBeInTheDocument();
    expect(within(rows[1]).queryByRole("button", { name: /revoke/i })).not.toBeInTheDocument();
    expect(within(rows[2]).getByText("expired")).toBeInTheDocument();
    expect(within(rows[3]).getByText("(you)")).toBeInTheDocument();
  });

  it("says so when there are no tokens", async () => {
    renderTokens({}, []);
    expect(await screen.findByText("No tokens")).toBeInTheDocument();
  });

  it("is for administrators only", async () => {
    const api = renderTokens({}, [token()], "viewer");
    expect(await screen.findByText("Administrators only")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /new token/i })).not.toBeInTheDocument();
    expect(api.called("GET", "/api/tokens")).toHaveLength(0);
  });

  it("creates a token, shows its secret once in a copyable box, and then never again", async () => {
    const secret = "fp_7.THE-ONE-TIME-SECRET";
    const api = renderTokens({
      "POST /api/tokens": {
        status: 201,
        body: envelope({ ...token({ id: 7, name: "ci-deploy", role: "admin" }), secret }),
      },
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /new token/i }));
    await user.type(dialog().getByLabelText("Name"), "  ci-deploy ");
    await user.selectOptions(dialog().getByLabelText("Role"), "admin");
    await user.selectOptions(dialog().getByLabelText("Expires"), "30");
    await user.click(dialog().getByRole("button", { name: "Create token" }));

    expect(api.called("POST", "/api/tokens")[0].body).toEqual({
      name: "ci-deploy",
      role: "admin",
      expiresInDays: 30,
    });
    expect(await screen.findByRole("heading", { name: "Token ci-deploy created" })).toBeInTheDocument();
    expect(dialog().getByLabelText("token")).toHaveValue(secret);
    expect(dialog().getByRole("alert")).toHaveTextContent("only time the token is shown");

    await user.click(dialog().getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("THE-ONE-TIME-SECRET");
  });

  it("checks the name before sending and shows the server's field errors", async () => {
    const api = renderTokens({
      "POST /api/tokens": apiError(400, "VALIDATION_FAILED", "validation failed", [
        { field: "name", message: "must be 1 to 63 characters" },
      ]),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /new token/i }));
    await user.click(dialog().getByRole("button", { name: "Create token" }));
    expect(dialog().getByText(/use 1 to 63 characters/i)).toBeInTheDocument();
    expect(api.called("POST", "/api/tokens")).toHaveLength(0);

    await user.type(dialog().getByLabelText("Name"), "ok-name");
    await user.click(dialog().getByRole("button", { name: "Create token" }));
    expect(await dialog().findByText("must be 1 to 63 characters")).toBeInTheDocument();
  });

  it("shows a duplicate name as an error above the form and keeps the dialog open", async () => {
    renderTokens({ "POST /api/tokens": apiError(409, "CONFLICT", 'a token named "ci" already exists') });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /new token/i }));
    await user.type(dialog().getByLabelText("Name"), "ci");
    await user.click(dialog().getByRole("button", { name: "Create token" }));
    expect(await dialog().findByText(/already exists/)).toBeInTheDocument();
    expect(dialog().getByRole("button", { name: "Create token" })).toBeEnabled();
  });

  it("revokes after a confirmation that says what happens", async () => {
    const api = renderTokens({ "DELETE /api/tokens/1": { body: { version: 9 } } });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Revoke ci" }));
    expect(dialog().getByText(/locked out at once/)).toBeInTheDocument();
    expect(dialog().queryByText(/signed in with/)).not.toBeInTheDocument();
    await user.click(dialog().getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(api.called("DELETE", "/api/tokens/1")).toHaveLength(1));
    expect(await screen.findByText("Revoked ci")).toBeInTheDocument();
  });

  it("warns before you revoke the token you are signed in with", async () => {
    renderTokens({}, [token({ id: 4, name: "test-admin", role: "admin" })]);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Revoke test-admin" }));
    expect(dialog().getByText(/you will be signed out/)).toBeInTheDocument();
  });

  it("shows the 409 for the last admin token", async () => {
    renderTokens({
      "DELETE /api/tokens/1": apiError(409, "CONFLICT", "the last usable admin token cannot be revoked"),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Revoke ci" }));
    await user.click(dialog().getByRole("button", { name: "Revoke" }));
    expect(await screen.findByText(/last usable admin token cannot be revoked/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
