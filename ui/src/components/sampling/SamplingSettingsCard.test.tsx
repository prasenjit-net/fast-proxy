import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { apiError, mockApi } from "../../test/mockApi";
import { envelope, samplingSettings, samplingView } from "../../test/fixtures";
import { renderPage } from "../../test/renderPage";
import SamplingSettingsCard from "./SamplingSettingsCard";

afterEach(() => {
  vi.unstubAllGlobals();
});

const render = (role: "admin" | "viewer" = "admin") =>
  renderPage(SamplingSettingsCard, { path: "/settings", role });

describe("SamplingSettingsCard", () => {
  it("shows the status, the stored values and the built-in redaction", async () => {
    mockApi({ "GET /api/sampling": { body: envelope(samplingView()) } });
    render();
    expect(await screen.findByText("42")).toBeInTheDocument();
    expect(screen.getByText("10 rows, 2.0 KiB")).toBeInTheDocument();
    expect(screen.getByLabelText(/Slow after/)).toHaveValue("2000");
    expect(screen.getByLabelText(/Large response from/)).toHaveValue("32");
    expect(screen.getByLabelText("Upstream unreachable", { exact: false })).toBeChecked();
    expect(screen.getByText("authorization")).toBeInTheDocument();
    expect(screen.getByText("password")).toBeInTheDocument();
  });

  it("sends the edited settings in the API's units and reports success", async () => {
    const api = mockApi({
      "GET /api/sampling": { body: envelope(samplingView()) },
      "PUT /api/sampling": { body: envelope(samplingSettings()) },
    });
    const user = userEvent.setup();
    render();
    const slow = await screen.findByLabelText(/Slow after/);
    await user.clear(slow);
    await user.type(slow, "750");
    await user.clear(screen.getByLabelText(/Keep for/));
    await user.type(screen.getByLabelText(/Keep for/), "14");
    await user.click(screen.getByLabelText(/Recovered by a retry/));
    await user.click(screen.getByRole("button", { name: "Save sampling settings" }));
    expect(await screen.findByText("Sampling settings saved")).toBeInTheDocument();
    const [put] = api.called("PUT", "/api/sampling");
    expect(put.body).toMatchObject({
      slowThresholdMs: 750,
      retentionDays: 14,
      slowP99FactorX10: 30,
      oversizedResponseBytes: 32 * 1024 * 1024,
      triggers: ["upstream_failure", "slow", "retried"],
      redactBodyFields: ["iban"],
    });
  });

  it("refuses an out-of-range value before sending anything", async () => {
    const api = mockApi({ "GET /api/sampling": { body: envelope(samplingView()) } });
    const user = userEvent.setup();
    render();
    const days = await screen.findByLabelText(/Keep for/);
    await user.clear(days);
    await user.type(days, "400");
    await user.click(screen.getByRole("button", { name: "Save sampling settings" }));
    expect(await screen.findByText(/from 1 to 365/)).toBeInTheDocument();
    expect(api.called("PUT", "/api/sampling")).toHaveLength(0);
  });

  it("shows the server's message next to the field it names", async () => {
    mockApi({
      "GET /api/sampling": { body: envelope(samplingView()) },
      "PUT /api/sampling": apiError(422, "VALIDATION", "invalid", [
        { field: "retentionDays", message: "too long for this database" },
      ]),
    });
    const user = userEvent.setup();
    render();
    await user.click(await screen.findByRole("button", { name: "Save sampling settings" }));
    expect(await screen.findByText("too long for this database")).toBeInTheDocument();
  });

  it("lets a viewer read the settings but not change them", async () => {
    mockApi({ "GET /api/sampling": { body: envelope(samplingView()) } });
    render("viewer");
    expect(await screen.findByLabelText(/Slow after/)).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: "Save sampling settings" })).not.toBeInTheDocument();
  });

  it("says when this process does not sample", async () => {
    mockApi({
      "GET /api/sampling": {
        body: envelope(samplingView({ status: { processEnabled: false, captured: 0, dropped: 0, table: null } })),
      },
    });
    render();
    expect(await screen.findByText(/does not sample/)).toBeInTheDocument();
    expect(screen.getByText("unknown")).toBeInTheDocument();
  });

  it("shows a load error", async () => {
    mockApi({});
    render();
    expect(await screen.findByText(/Could not load the sampling settings/)).toBeInTheDocument();
  });
});
