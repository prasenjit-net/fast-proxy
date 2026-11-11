import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, maintenanceScope } from "../../test/fixtures";
import { apiError, mockApi, type Reply } from "../../test/mockApi";
import { renderPage } from "../../test/renderPage";
import type { MaintenanceTarget } from "../../lib/types";
import MaintenanceControl from "./MaintenanceControl";

// The clock is read once per render here; ticking is tested with Countdown.
vi.mock("../../lib/useNow", () => ({ useNow: () => Date.now() }));

const LIST = "GET /api/maintenance";
const SHOP: MaintenanceTarget = { scope: "subsystem", subsystemId: 1, name: "shop" };
const dialog = () => within(screen.getByRole("dialog"));

function renderControl(
  target: MaintenanceTarget,
  handlers: Record<string, Reply> = {},
  scopes = [] as ReturnType<typeof maintenanceScope>[],
  role: "admin" | "viewer" = "admin",
) {
  const api = mockApi({ [LIST]: { body: envelope(scopes) }, ...handlers });
  const view = renderPage(() => <MaintenanceControl target={target} />, { path: "/control", role });
  // "open" is also what shows before the list arrives, so tests that expect it wait for the list.
  const listed = () =>
    waitFor(() => expect(view.client.getQueryState(["maintenance"])?.status).toBe("success"));
  return Object.assign(api, { listed });
}

const shopScope = (changes = {}) =>
  maintenanceScope({ scope: "subsystem", subsystemId: 1, subsystem: "shop", ...changes });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("MaintenanceControl", () => {
  it("shows a scope that is open, with a button to turn it on", async () => {
    const api = renderControl(SHOP);
    await api.listed();
    expect(await screen.findByText("open")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /turn on maintenance for subsystem shop/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /turn off/i })).not.toBeInTheDocument();
  });

  it("shows the message and the time left for a scope that is on", async () => {
    renderControl(SHOP, {}, [shopScope({ expiresAtMs: Date.now() + 65_000 })]);
    expect(await screen.findByText("in maintenance")).toBeInTheDocument();
    expect(screen.getByText(/ends in 1:0[45]/)).toBeInTheDocument();
    expect(screen.getByText(/Back at 14:00/)).toBeInTheDocument();
  });

  it("says until turned off when there is no end, and ignores a scope for something else", async () => {
    renderControl(SHOP, {}, [
      shopScope({ expiresAtMs: null }),
      maintenanceScope({ scope: "subsystem", subsystemId: 2, subsystem: "blog" }),
    ]);
    expect(await screen.findByText(/until turned off/)).toBeInTheDocument();
  });

  it("treats a scope whose time has passed as open, without waiting for the server", async () => {
    const api = renderControl(SHOP, {}, [shopScope({ expiresAtMs: Date.now() - 1000 })]);
    await api.listed();
    expect(screen.getByText("open")).toBeInTheDocument();
    expect(screen.queryByText("in maintenance")).not.toBeInTheDocument();
  });

  it("turns maintenance on with the exact body, then shows the generated secret once", async () => {
    const secret = "generated-bypass-secret-123";
    const api = renderControl(SHOP, {
      "PUT /api/subsystems/1/maintenance": {
        body: { data: shopScope(), version: 5, bypassSecret: secret },
      },
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn on maintenance/i }));
    await user.type(dialog().getByLabelText("Message"), "Deploying v2");
    await user.selectOptions(dialog().getByLabelText("How long"), "3600");
    await user.type(dialog().getByLabelText("Networks"), "10.0.0.0/8");
    await user.type(dialog().getByLabelText("Bypass header"), "X-Bypass");
    await user.click(dialog().getByRole("button", { name: /turn on for subsystem shop/i }));

    await waitFor(() => expect(api.called("PUT", "/api/subsystems/1/maintenance")).toHaveLength(1));
    expect(api.called("PUT", "/api/subsystems/1/maintenance")[0].body).toEqual({
      message: "Deploying v2",
      ttlSecs: 3600,
      bypass: { cidrs: ["10.0.0.0/8"], header: "X-Bypass" },
    });
    expect(await dialog().findByLabelText("bypass secret")).toHaveValue(secret);
    expect(dialog().getByRole("alert")).toHaveTextContent(/only time/i);

    await user.click(dialog().getByRole("button", { name: "Done" }));
    expect(screen.queryByDisplayValue(secret)).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(secret);
  });

  it("shows no secret box when none was generated", async () => {
    const api = renderControl(SHOP, {
      "PUT /api/subsystems/1/maintenance": { body: { data: shopScope(), version: 5, bypassSecret: null } },
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn on maintenance/i }));
    await user.click(dialog().getByRole("button", { name: /turn on for/i }));
    await waitFor(() => expect(api.called("PUT", "/api/subsystems/1/maintenance")).toHaveLength(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByLabelText("bypass secret")).not.toBeInTheDocument();
  });

  it("shows a client-side error and sends nothing when the input is invalid", async () => {
    const api = renderControl(SHOP);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn on maintenance/i }));
    await user.type(dialog().getByLabelText("Networks"), "not-a-network");
    await user.click(dialog().getByRole("button", { name: /turn on for/i }));
    expect(await dialog().findByText(/networks such as 10\.0\.0\.0\/8/i)).toBeInTheDocument();
    expect(api.called("PUT", "/api/subsystems")).toHaveLength(0);
  });

  it("shows the server's field message beside the field", async () => {
    renderControl(SHOP, {
      "PUT /api/subsystems/1/maintenance": apiError(422, "VALIDATION", "invalid", [
        { field: "bypass.cidrs", message: "10.0.0.0/99 is not a network" },
      ]),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn on maintenance/i }));
    await user.type(dialog().getByLabelText("Networks"), "10.0.0.0/8");
    await user.click(dialog().getByRole("button", { name: /turn on for/i }));
    expect(await dialog().findByText("10.0.0.0/99 is not a network")).toBeInTheDocument();
  });

  it("asks before turning off, says what reopens, and sends the delete only on confirm", async () => {
    const api = renderControl(SHOP, { "DELETE /api/subsystems/1/maintenance": { body: { version: 6 } } }, [
      shopScope(),
    ]);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn off maintenance for subsystem shop/i }));
    expect(dialog().getByText(/hosts answer normally again/i)).toBeInTheDocument();
    expect(api.called("DELETE", "/api/subsystems")).toHaveLength(0);

    await user.click(dialog().getByRole("button", { name: "Cancel" }));
    expect(api.called("DELETE", "/api/subsystems")).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: /turn off maintenance/i }));
    await user.click(dialog().getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(api.called("DELETE", "/api/subsystems/1/maintenance")).toHaveLength(1));
  });

  it.each([
    [{ scope: "global" } as MaintenanceTarget, "/api/maintenance/global", /every host on this proxy/i],
    [
      { scope: "route", subsystemId: 1, routeId: 7, prefix: "/api" } as MaintenanceTarget,
      "/api/subsystems/1/routes/7/maintenance",
      /requests under that path/i,
    ],
  ])("targets the right URL and explains the right scope for %j", async (target, path, explanation) => {
    const scope =
      target.scope === "global" ? maintenanceScope() : maintenanceScope({ scope: "route", routeId: 7, subsystemId: 1 });
    const api = renderControl(target, { [`DELETE ${path}`]: { body: { version: 6 } } }, [scope]);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /turn off/i }));
    expect(dialog().getByText(explanation)).toBeInTheDocument();
    await user.click(dialog().getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(api.called("DELETE", path)).toHaveLength(1));
  });

  it("shows a viewer the state and the countdown but no control", async () => {
    renderControl(SHOP, {}, [shopScope()], "viewer");
    expect(await screen.findByText("in maintenance")).toBeInTheDocument();
    expect(screen.getByText(/ends in/)).toBeInTheDocument();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });
});
