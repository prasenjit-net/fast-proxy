import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, samplingOverride, samplingView } from "../../test/fixtures";
import { mockApi } from "../../test/mockApi";
import { renderPage } from "../../test/renderPage";
import SamplingControl from "./SamplingControl";

afterEach(() => {
  vi.unstubAllGlobals();
});

const render = (target: { routeId: number } | { subsystemId: number }, role: "admin" | "viewer" = "admin") =>
  renderPage(() => <SamplingControl target={target} label="route /api" />, { path: "/x", role });

describe("SamplingControl", () => {
  it("says when a route inherits, and what an override changes", async () => {
    mockApi({
      "GET /api/sampling": {
        body: envelope(samplingView({ overrides: [samplingOverride({ routeId: 7, captureBodies: true, baselinePerMille: 20 })] })),
      },
    });
    const first = render({ routeId: 8 });
    expect(await screen.findByText("sampling: inherits")).toBeInTheDocument();
    first.unmount();
    render({ routeId: 7 });
    expect(await screen.findByText("sampling: bodies on, baseline 20‰")).toBeInTheDocument();
  });

  it("sets an override with only the chosen fields, warning when bodies go on", async () => {
    const api = mockApi({
      "GET /api/sampling": { body: envelope(samplingView()) },
      "PUT /api/sampling/overrides": { body: envelope(samplingOverride()) },
    });
    const user = userEvent.setup();
    render({ routeId: 7 });
    await user.click(await screen.findByRole("button", { name: "Edit sampling for route /api" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("note")).not.toBeInTheDocument();
    await user.selectOptions(within(dialog).getByLabelText("Keep request and response bodies"), "on");
    expect(within(dialog).getByRole("note")).toHaveTextContent("personal data");
    await user.type(within(dialog).getByLabelText(/Baseline/), "50");
    await user.click(within(dialog).getByRole("button", { name: "Save override" }));
    expect(await screen.findByText("Sampling override saved")).toBeInTheDocument();
    expect(api.called("PUT", "/api/sampling/overrides")[0].body).toEqual({
      routeId: 7,
      captureBodies: true,
      baselinePerMille: 50,
    });
  });

  it("clears the override when everything is set back to inherit", async () => {
    const api = mockApi({
      "GET /api/sampling": { body: envelope(samplingView({ overrides: [samplingOverride({ enabled: false })] })) },
      "DELETE /api/sampling/overrides?routeId=7": { body: { version: 5 } },
    });
    const user = userEvent.setup();
    render({ routeId: 7 });
    await user.click(await screen.findByRole("button", { name: "Edit sampling for route /api" }));
    const dialog = screen.getByRole("dialog");
    await user.selectOptions(within(dialog).getByLabelText("Sampling"), "inherit");
    await user.click(within(dialog).getByRole("button", { name: "Clear override" }));
    expect(await screen.findByText("Sampling override saved")).toBeInTheDocument();
    expect(api.called("DELETE", "/api/sampling/overrides?routeId=7")).toHaveLength(1);
    expect(api.called("PUT", "/api/sampling/overrides")).toHaveLength(0);
  });

  it("targets a subsystem by its id", async () => {
    const api = mockApi({
      "GET /api/sampling": { body: envelope(samplingView()) },
      "PUT /api/sampling/overrides": { body: envelope(samplingOverride()) },
    });
    const user = userEvent.setup();
    render({ subsystemId: 3 });
    await user.click(await screen.findByRole("button", { name: "Edit sampling for route /api" }));
    await user.selectOptions(screen.getByLabelText("Sampling"), "off");
    await user.click(screen.getByRole("button", { name: "Save override" }));
    await screen.findByText("Sampling override saved");
    expect(api.called("PUT", "/api/sampling/overrides")[0].body).toEqual({ subsystemId: 3, enabled: false });
  });

  it("refuses a bad number before sending", async () => {
    const api = mockApi({ "GET /api/sampling": { body: envelope(samplingView()) } });
    const user = userEvent.setup();
    render({ routeId: 7 });
    await user.click(await screen.findByRole("button", { name: "Edit sampling for route /api" }));
    await user.type(screen.getByLabelText(/Baseline/), "5000");
    await user.click(screen.getByRole("button", { name: "Save override" }));
    expect(await screen.findByText(/from 0 to 1,000/)).toBeInTheDocument();
    expect(api.called("PUT", "/api/sampling/overrides")).toHaveLength(0);
  });

  it("shows a viewer the badge but no way to edit", async () => {
    mockApi({ "GET /api/sampling": { body: envelope(samplingView()) } });
    render({ routeId: 7 }, "viewer");
    expect(await screen.findByText("sampling: inherits")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Edit sampling/ })).not.toBeInTheDocument();
  });

  it("renders nothing while the settings cannot be read", async () => {
    mockApi({});
    const view = render({ routeId: 7 });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(view.container).toHaveTextContent("");
  });
});
