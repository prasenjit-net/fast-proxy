import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultPolicyValues, type PolicyValues } from "../../lib/policyValidation";
import { caBundle, envelope } from "../../test/fixtures";
import { mockApi } from "../../test/mockApi";
import UpstreamTlsFields from "./UpstreamTlsFields";

function Harness({ initial, errors = {} }: { initial?: Partial<PolicyValues>; errors?: Record<string, string> }) {
  const [values, setValues] = useState<PolicyValues>({ ...defaultPolicyValues(), ...initial });
  return (
    <>
      <UpstreamTlsFields
        values={values}
        onChange={(patch) => setValues((current) => ({ ...current, ...patch }))}
        error={(field) => errors[field]}
        idPrefix="up"
      />
      <output data-testid="values">{JSON.stringify(values)}</output>
    </>
  );
}

function renderFields(
  initial: Partial<PolicyValues> = {},
  options: { errors?: Record<string, string> } = {},
) {
  mockApi({
    "GET /api/ca-bundles": { body: envelope([caBundle({ name: "corp" }), caBundle({ id: 3, name: "partners" })]) },
  });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <Harness initial={initial} errors={options.errors} />
    </QueryClientProvider>,
  );
}

const values = () => JSON.parse(screen.getByTestId("values").textContent ?? "{}") as PolicyValues;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("UpstreamTlsFields", () => {
  it("shows only the switch while TLS is off", () => {
    renderFields();
    expect(screen.getByLabelText("Connect to the endpoints over TLS")).not.toBeChecked();
    expect(screen.queryByLabelText("Server name")).not.toBeInTheDocument();
  });

  it("reveals the verification, name and CA fields once TLS is on", async () => {
    renderFields();
    await userEvent.click(screen.getByLabelText("Connect to the endpoints over TLS"));
    expect(values().tlsEnabled).toBe(true);
    expect(screen.getByLabelText("Verify the endpoint's certificate")).toBeChecked();
    expect(screen.queryByText(/not verified/)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Server name")).toBeInTheDocument();
    expect(await screen.findByRole("option", { name: "corp" })).toBeInTheDocument();
  });

  it("warns, with a badge, when verification is off", async () => {
    renderFields({ tlsEnabled: true });
    await userEvent.click(screen.getByLabelText("Verify the endpoint's certificate"));
    expect(values().tlsVerify).toBe(false);
    expect(screen.getByText(/not verified: anyone can impersonate the endpoint/)).toBeInTheDocument();
  });

  it("chooses a CA bundle by name, defaulting to the OS trust store", async () => {
    renderFields({ tlsEnabled: true });
    await screen.findByRole("option", { name: "partners" });
    expect(screen.getByLabelText("CA bundle")).toHaveValue("");
    await userEvent.selectOptions(screen.getByLabelText("CA bundle"), "partners");
    expect(values().tlsCa).toBe("partners");
  });

  it("keeps a saved CA name that no longer exists visible instead of dropping it", async () => {
    renderFields({ tlsEnabled: true, tlsCa: "gone" });
    expect(await screen.findByRole("option", { name: "gone (not found)" })).toBeInTheDocument();
  });

  it("shows the server's errors beside the fields", async () => {
    renderFields(
      { tlsEnabled: true },
      { errors: { "tls.serverName": "is not a valid server name", "tls.ca": "no CA bundle has this name" } },
    );
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.map((alert) => alert.textContent)).toEqual([
      "is not a valid server name",
      "no CA bundle has this name",
    ]);
  });

  it("types a server name", async () => {
    renderFields({ tlsEnabled: true });
    await userEvent.type(screen.getByLabelText("Server name"), "backend.internal");
    expect(values().tlsServerName).toBe("backend.internal");
  });
});
