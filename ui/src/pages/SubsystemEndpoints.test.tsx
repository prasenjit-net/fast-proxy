// The endpoints of an upstream group on the subsystem page: their live
// state, adding, editing and removing them, and enabling, draining and
// disabling them with a confirmation that says what will happen.
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  endpoint,
  endpointStatus,
  envelope,
  subsystemDetail,
  upstream,
  upstreamStatus,
} from "../test/fixtures";
import { apiError, mockApi, type Reply } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import SubsystemDetailPage from "./SubsystemDetail";

const DETAIL = "GET /api/subsystems/1";
const STATUS = "GET /api/subsystems/1/upstreams/5/status";
const BASE = "/api/subsystems/1/upstreams/5/endpoints";
const written: Reply = { body: { version: 5 } };

const three = [
  endpoint({ id: 11, addr: "10.0.0.1:80" }),
  endpoint({ id: 12, addr: "10.0.0.2:80", weight: 3 }),
  endpoint({ id: 13, addr: "10.0.0.3:80", state: "draining" }),
];

const liveThree = upstreamStatus([
  endpointStatus({ id: 11, addr: "10.0.0.1:80", inFlight: 4, lastProbe: { ok: true, agoMs: 2000 } }),
  endpointStatus({
    id: 12,
    addr: "10.0.0.2:80",
    weight: 3,
    effective: "unhealthy",
    reason: "failed health checks",
    lastProbe: { ok: false, agoMs: 3000 },
  }),
  endpointStatus({
    id: 13,
    addr: "10.0.0.3:80",
    state: "draining",
    effective: "draining",
    reason: "draining: no new requests",
    inFlight: 2,
  }),
]);

function renderEndpoints(handlers: Record<string, Reply>, endpoints = three, live = liveThree) {
  const detail = subsystemDetail({ upstreams: [upstream({ endpoints })] });
  const api = mockApi({
    [DETAIL]: { body: envelope(detail) },
    [STATUS]: { body: envelope(live) },
    ...handlers,
  });
  renderPage(SubsystemDetailPage, { path: "/subsystems/$id", entry: "/subsystems/1" });
  return api;
}

const dialog = () => within(screen.getByRole("dialog"));
const rowOf = async (addr: string) => within((await screen.findByText(addr)).closest("tr") as HTMLElement);

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("endpoint table", () => {
  it("shows each endpoint's weight, live state, load and last check", async () => {
    renderEndpoints({});

    const first = await rowOf("10.0.0.1:80");
    expect(await first.findByText("healthy")).toBeInTheDocument();
    expect(first.getByText("4")).toBeInTheDocument();
    expect(first.getByText("passed 2s ago")).toBeInTheDocument();

    const second = await rowOf("10.0.0.2:80");
    expect(second.getByText("3")).toBeInTheDocument();
    expect(await second.findByText("unhealthy")).toBeInTheDocument();
    expect(second.getByText("failed 3s ago")).toBeInTheDocument();

    const third = await rowOf("10.0.0.3:80");
    expect(await third.findByText("draining")).toBeInTheDocument();
  });

  it("summarizes how many endpoints are healthy", async () => {
    renderEndpoints({});
    expect(await screen.findByText("1 of 3 healthy")).toBeInTheDocument();
    expect(screen.getByText("Round robin")).toBeInTheDocument();
  });

  it("colours the health summary by how many endpoints are in rotation", async () => {
    const tone = async (statuses: Parameters<typeof upstreamStatus>[0]) => {
      mockApi({
        [DETAIL]: { body: envelope(subsystemDetail({ upstreams: [upstream({ endpoints: three })] })) },
        [STATUS]: { body: envelope(upstreamStatus(statuses)) },
      });
      const { unmount } = renderPage(SubsystemDetailPage, { path: "/subsystems/$id", entry: "/subsystems/1" });
      const badge = await screen.findByText(/\d of 3 healthy/);
      const className = badge.className;
      unmount();
      return className;
    };
    const down = (id: number) => endpointStatus({ id, addr: `10.0.0.${id - 10}:80`, effective: "unhealthy" });
    const up = (id: number) => endpointStatus({ id, addr: `10.0.0.${id - 10}:80` });
    expect(await tone([up(11), up(12), up(13)])).toContain("text-ok");
    expect(await tone([up(11), down(12), down(13)])).toContain("text-warn");
    expect(await tone([down(11), down(12), down(13)])).toContain("text-err");
  });

  it("says a draining endpoint is drained once nothing is left on it", async () => {
    const drained = upstreamStatus([
      endpointStatus({ id: 13, addr: "10.0.0.3:80", state: "draining", effective: "draining", inFlight: 0 }),
    ]);
    renderEndpoints({}, [three[2]], drained);
    expect(await screen.findByText("drained")).toBeInTheDocument();
  });

  it("still lists the endpoints with their configured state when live status is unavailable", async () => {
    renderEndpoints({ [STATUS]: apiError(500, "INTERNAL", "boom") });
    const first = await rowOf("10.0.0.1:80");
    expect(first.getByText("enabled")).toBeInTheDocument();
    expect(first.getByText("no checks")).toBeInTheDocument();
    expect(screen.queryByText(/healthy$/)).not.toBeInTheDocument();
  });
});

describe("adding, editing and removing endpoints", () => {
  it("adds an endpoint with its weight", async () => {
    const api = renderEndpoints({ [`POST ${BASE}`]: { status: 201, body: envelope(endpoint(), 5) } });
    await userEvent.click(await screen.findByRole("button", { name: /add endpoint/i }));
    await userEvent.type(dialog().getByLabelText("Address"), "10.0.0.9:80");
    await userEvent.clear(dialog().getByLabelText("Weight"));
    await userEvent.type(dialog().getByLabelText("Weight"), "5");
    await userEvent.click(dialog().getByRole("button", { name: "Add endpoint" }));

    expect(await screen.findByText("Added endpoint 10.0.0.9:80")).toBeInTheDocument();
    expect(api.called("POST", BASE)[0].body).toEqual({ addr: "10.0.0.9:80", weight: 5 });
  });

  it("checks the form before sending", async () => {
    const api = renderEndpoints({});
    await userEvent.click(await screen.findByRole("button", { name: /add endpoint/i }));
    await userEvent.type(dialog().getByLabelText("Address"), "nope");
    await userEvent.clear(dialog().getByLabelText("Weight"));
    await userEvent.type(dialog().getByLabelText("Weight"), "0");
    await userEvent.click(dialog().getByRole("button", { name: "Add endpoint" }));
    const alerts = await dialog().findAllByRole("alert");
    expect(alerts.map((a) => a.textContent)).toEqual([
      "Use host:port, with a port from 1 to 65535",
      "Must be a whole number from 1 to 1000",
    ]);
    expect(api.called("POST", BASE)).toHaveLength(0);
  });

  it("shows a duplicate address inside the dialog", async () => {
    renderEndpoints({ [`POST ${BASE}`]: apiError(409, "CONFLICT", "already exists") });
    await userEvent.click(await screen.findByRole("button", { name: /add endpoint/i }));
    await userEvent.type(dialog().getByLabelText("Address"), "10.0.0.1:80");
    await userEvent.click(dialog().getByRole("button", { name: "Add endpoint" }));
    expect(await dialog().findByRole("alert")).toHaveTextContent("already exists");
  });

  it("edits an endpoint, prefilled", async () => {
    const api = renderEndpoints({ [`PUT ${BASE}/12`]: { body: envelope(endpoint(), 5) } });
    await userEvent.click(await screen.findByRole("button", { name: "Edit endpoint 10.0.0.2:80" }));
    expect(dialog().getByLabelText("Address")).toHaveValue("10.0.0.2:80");
    expect(dialog().getByLabelText("Weight")).toHaveValue("3");
    await userEvent.clear(dialog().getByLabelText("Weight"));
    await userEvent.type(dialog().getByLabelText("Weight"), "7");
    await userEvent.click(dialog().getByRole("button", { name: "Save endpoint" }));

    expect(await screen.findByText("Updated endpoint 10.0.0.2:80")).toBeInTheDocument();
    expect(api.called("PUT", `${BASE}/12`)[0].body).toEqual({ addr: "10.0.0.2:80", weight: 7 });
  });

  it("asks before removing an endpoint and then removes it", async () => {
    const api = renderEndpoints({ [`DELETE ${BASE}/12`]: written });
    await userEvent.click(await screen.findByRole("button", { name: "Remove endpoint 10.0.0.2:80" }));
    expect(dialog().getByText(/requests already on it finish/i)).toBeInTheDocument();
    await userEvent.click(dialog().getByRole("button", { name: "Remove endpoint" }));
    expect(await screen.findByText("Removed endpoint 10.0.0.2:80")).toBeInTheDocument();
    expect(api.called("DELETE", `${BASE}/12`)).toHaveLength(1);
  });

  it("cannot remove the last endpoint of a group", async () => {
    renderEndpoints({}, [three[0]], upstreamStatus([endpointStatus({ id: 11, addr: "10.0.0.1:80" })]));
    const button = await screen.findByRole("button", { name: "Remove endpoint 10.0.0.1:80" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", expect.stringMatching(/at least one endpoint/i));
  });
});

describe("enabling, draining and disabling", () => {
  it("drains after a confirmation that says what will happen and how much is in flight", async () => {
    const api = renderEndpoints({
      [`PUT ${BASE}/11/state`]: { body: envelope(endpoint({ state: "draining" }), 5) },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Drain endpoint 10.0.0.1:80" }));

    expect(dialog().getByText(/stops receiving new requests/i)).toBeInTheDocument();
    expect(dialog().getByText(/shows as drained when nothing is left/i)).toBeInTheDocument();
    expect(dialog().getByText(/4 request\(s\) are on it now/i)).toBeInTheDocument();
    expect(api.called("PUT", `${BASE}/11/state`)).toHaveLength(0);

    await userEvent.click(dialog().getByRole("button", { name: "Drain endpoint" }));
    expect(await screen.findByText("Endpoint 10.0.0.1:80 is now draining")).toBeInTheDocument();
    expect(api.called("PUT", `${BASE}/11/state`)[0].body).toEqual({ state: "draining" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("disables after a confirmation, and can be cancelled", async () => {
    const api = renderEndpoints({
      [`PUT ${BASE}/11/state`]: { body: envelope(endpoint({ state: "disabled" }), 5) },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Disable endpoint 10.0.0.1:80" }));
    expect(dialog().getByText(/stays configured, parked/i)).toBeInTheDocument();
    await userEvent.click(dialog().getByRole("button", { name: "Cancel" }));
    expect(api.called("PUT", `${BASE}/11/state`)).toHaveLength(0);

    await userEvent.click(screen.getByRole("button", { name: "Disable endpoint 10.0.0.1:80" }));
    await userEvent.click(dialog().getByRole("button", { name: "Disable endpoint" }));
    expect(await screen.findByText("Endpoint 10.0.0.1:80 is now disabled")).toBeInTheDocument();
    expect(api.called("PUT", `${BASE}/11/state`)[0].body).toEqual({ state: "disabled" });
  });

  it("enables a draining endpoint at once, without a confirmation", async () => {
    const api = renderEndpoints({
      [`PUT ${BASE}/13/state`]: { body: envelope(endpoint({ state: "enabled" }), 5) },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Enable endpoint 10.0.0.3:80" }));
    expect(await screen.findByText("Endpoint 10.0.0.3:80 is now enabled")).toBeInTheDocument();
    expect(api.called("PUT", `${BASE}/13/state`)[0].body).toEqual({ state: "enabled" });
  });

  it("offers drain and disable only for enabled endpoints and enable only for the others", async () => {
    renderEndpoints({});
    await screen.findByText("10.0.0.3:80");
    expect(screen.getByRole("button", { name: "Drain endpoint 10.0.0.1:80" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable endpoint 10.0.0.1:80" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Drain endpoint 10.0.0.3:80" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Enable endpoint 10.0.0.3:80" })).toBeInTheDocument();
  });

  it("shows the server's refusal as a notification", async () => {
    renderEndpoints({ [`PUT ${BASE}/11/state`]: apiError(404, "NOT_FOUND", "endpoint 11 not found") });
    await userEvent.click(await screen.findByRole("button", { name: "Drain endpoint 10.0.0.1:80" }));
    await userEvent.click(dialog().getByRole("button", { name: "Drain endpoint" }));
    expect(await screen.findByText("endpoint 11 not found")).toBeInTheDocument();
  });
});
