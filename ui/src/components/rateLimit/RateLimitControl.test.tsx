import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RateLimit, RateLimitTarget } from "../../lib/types";
import { envelope, rateLimit } from "../../test/fixtures";
import { apiError, mockApi } from "../../test/mockApi";
import { renderPage } from "../../test/renderPage";
import RateLimitControl from "./RateLimitControl";

afterEach(() => {
  vi.unstubAllGlobals();
});

const ROUTE: RateLimitTarget = { subsystemId: 1, routeId: 7 };
const ROUTE_PATH = "/api/subsystems/1/routes/7/rate-limit";

function render(props: {
  target?: RateLimitTarget;
  current?: RateLimit | null;
  inherited?: RateLimit | null;
  role?: "admin" | "viewer";
}) {
  return renderPage(
    () => (
      <RateLimitControl
        target={props.target ?? ROUTE}
        current={props.current ?? null}
        inherited={props.inherited}
        label="route /api"
      />
    ),
    { path: "/x", role: props.role ?? "admin" },
  );
}

const dialog = () => within(screen.getByRole("dialog"));
const open = async () =>
  userEvent.click(await screen.findByRole("button", { name: "Edit rate limit for route /api" }));

describe("RateLimitControl", () => {
  it("says what applies: none, the subsystem's, or the route's own", async () => {
    mockApi({});
    const none = render({});
    expect(await screen.findByText("no rate limit")).toBeInTheDocument();
    none.unmount();
    const inherited = render({ inherited: rateLimit() });
    expect(await screen.findByText("subsystem's: 10 / min")).toBeInTheDocument();
    inherited.unmount();
    render({ current: rateLimit({ periodSecs: 1, burst: 20 }), inherited: rateLimit() });
    expect(await screen.findByText("10 / s, burst 20")).toBeInTheDocument();
  });

  it("sets a policy keyed on a header, saying it is per instance", async () => {
    const api = mockApi({ [`PUT ${ROUTE_PATH}`]: { body: envelope(rateLimit()) } });
    render({});
    await open();
    expect(dialog().getByText(/each instance separately/)).toBeInTheDocument();
    const rate = dialog().getByLabelText("Requests (per instance)");
    await userEvent.clear(rate);
    await userEvent.type(rate, "100");
    await userEvent.clear(dialog().getByLabelText("Every"));
    await userEvent.type(dialog().getByLabelText("Every"), "5");
    await userEvent.selectOptions(dialog().getByLabelText("Period unit"), "m");
    await userEvent.type(dialog().getByLabelText("Burst"), "200");
    await userEvent.selectOptions(dialog().getByLabelText("Count per"), "header");
    await userEvent.type(dialog().getByLabelText("Header name"), "X-Customer");
    await userEvent.click(dialog().getByRole("button", { name: "Save limit" }));

    expect(await screen.findByText("Rate limit saved for route /api")).toBeInTheDocument();
    expect(api.called("PUT", ROUTE_PATH)[0].body).toEqual({
      rate: 100,
      periodSecs: 300,
      burst: 200,
      key: "header",
      header: "x-customer",
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("targets the subsystem's own policy", async () => {
    const api = mockApi({ "PUT /api/subsystems/1/rate-limit": { body: envelope(rateLimit()) } });
    render({ target: { subsystemId: 1 } });
    await open();
    await userEvent.click(dialog().getByRole("button", { name: "Save limit" }));
    await screen.findByText("Rate limit saved for route /api");
    expect(api.called("PUT", "/api/subsystems/1/rate-limit")[0].body).toEqual({ rate: 10, periodSecs: 1 });
  });

  it("prefills the current policy and removes it", async () => {
    const api = mockApi({ [`DELETE ${ROUTE_PATH}`]: { body: { version: 6 } } });
    render({ current: rateLimit({ periodSecs: 3600, burst: 3 }) });
    await open();
    expect(dialog().getByLabelText("Requests (per instance)")).toHaveValue("10");
    expect(dialog().getByLabelText("Period unit")).toHaveValue("h");
    expect(dialog().getByLabelText("Burst")).toHaveValue("3");
    await userEvent.click(dialog().getByRole("button", { name: "Remove limit" }));
    expect(await screen.findByText("Rate limit removed from route /api")).toBeInTheDocument();
    expect(api.called("DELETE", ROUTE_PATH)).toHaveLength(1);
  });

  it("offers no removal when there is nothing to remove", async () => {
    mockApi({});
    render({});
    await open();
    expect(dialog().queryByRole("button", { name: "Remove limit" })).not.toBeInTheDocument();
  });

  it("checks the form before sending and shows the server's field errors inline", async () => {
    const api = mockApi({
      [`PUT ${ROUTE_PATH}`]: apiError(400, "VALIDATION_FAILED", "validation failed", [
        { field: "rateLimit.burst", message: "must be between 1 and 10000" },
      ]),
    });
    render({});
    await open();
    await userEvent.clear(dialog().getByLabelText("Requests (per instance)"));
    await userEvent.type(dialog().getByLabelText("Requests (per instance)"), "0");
    await userEvent.selectOptions(dialog().getByLabelText("Count per"), "header");
    await userEvent.type(dialog().getByLabelText("Header name"), "Host");
    await userEvent.click(dialog().getByRole("button", { name: "Save limit" }));
    expect(dialog().getByLabelText("Requests (per instance)")).toHaveAttribute("aria-invalid", "true");
    expect(dialog().getByText(/describes the connection/)).toBeInTheDocument();
    expect(api.called("PUT", ROUTE_PATH)).toHaveLength(0);

    await userEvent.clear(dialog().getByLabelText("Requests (per instance)"));
    await userEvent.type(dialog().getByLabelText("Requests (per instance)"), "5");
    await userEvent.clear(dialog().getByLabelText("Header name"));
    await userEvent.type(dialog().getByLabelText("Header name"), "x-customer");
    await userEvent.click(dialog().getByRole("button", { name: "Save limit" }));
    expect(await dialog().findByText("must be between 1 and 10000")).toBeInTheDocument();
    expect(dialog().getByLabelText("Burst")).toHaveAttribute("aria-invalid", "true");
  });

  it("shows a viewer what applies and no way to change it", async () => {
    mockApi({});
    render({ current: rateLimit(), role: "viewer" });
    expect(await screen.findByText("10 / min")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Edit rate limit/ })).not.toBeInTheDocument();
  });
});
