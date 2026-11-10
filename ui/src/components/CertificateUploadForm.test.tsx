import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../lib/http";
import CertificateUploadForm from "./CertificateUploadForm";

const cert = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
const key = "-----BEGIN PRIVATE KEY-----\nBBBB\n-----END PRIVATE KEY-----";

function setup(props: Partial<React.ComponentProps<typeof CertificateUploadForm>> = {}) {
  const onSubmit = vi.fn().mockResolvedValue(undefined);
  const onCancel = vi.fn();
  render(<CertificateUploadForm submitLabel="Upload" onSubmit={onSubmit} onCancel={onCancel} {...props} />);
  return { onSubmit, onCancel };
}

async function fill(name: string, certPem: string, keyPem: string) {
  if (name) await userEvent.type(screen.getByLabelText("Name"), name);
  await userEvent.click(screen.getByLabelText("Certificate chain (PEM)"));
  await userEvent.paste(certPem);
  await userEvent.click(screen.getByLabelText("Private key (PEM)"));
  await userEvent.paste(keyPem);
}

describe("CertificateUploadForm", () => {
  it("checks the form before sending and names each problem", async () => {
    const { onSubmit } = setup();
    await userEvent.click(screen.getByRole("button", { name: "Upload" }));
    const alerts = (await screen.findAllByRole("alert")).map((alert) => alert.textContent);
    expect(alerts).toHaveLength(3);
    expect(alerts.join(" ")).toMatch(/BEGIN CERTIFICATE/);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("sends the name, chain and key, then clears the key it was given", async () => {
    const { onSubmit } = setup();
    await fill("shop", cert, key);
    await userEvent.click(screen.getByRole("button", { name: "Upload" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ name: "shop", certPem: cert, keyPem: key }));
    await waitFor(() => expect(screen.getByLabelText("Private key (PEM)")).toHaveValue(""));
    expect(screen.getByLabelText("Certificate chain (PEM)")).toHaveValue(cert);
  });

  it("never renders the key anywhere but its own field", async () => {
    setup();
    await userEvent.click(screen.getByLabelText("Private key (PEM)"));
    await userEvent.paste(key);
    const copy = document.body.cloneNode(true) as HTMLElement;
    copy.querySelectorAll("textarea").forEach((field) => field.remove());
    expect(copy.textContent).not.toContain("BBBB");
  });

  it("reads a chosen file in the browser into its field", async () => {
    setup();
    const file = new File([cert], "chain.pem", { type: "application/x-pem-file" });
    await userEvent.upload(screen.getByLabelText("Choose a certificate file"), file);
    await waitFor(() => expect(screen.getByLabelText("Certificate chain (PEM)")).toHaveValue(cert));
  });

  it("shows the server's field errors inline and keeps the key field as typed", async () => {
    const onSubmit = vi
      .fn()
      .mockRejectedValue(new ApiError("VALIDATION_FAILED", 400, "validation failed", [
        { field: "keyPem", message: "does not match the certificate" },
      ]));
    setup({ onSubmit });
    await fill("shop", cert, key);
    await userEvent.click(screen.getByRole("button", { name: "Upload" }));
    expect(await screen.findByText("does not match the certificate")).toBeInTheDocument();
    expect(screen.getByLabelText("Private key (PEM)")).toHaveValue(key);
  });

  it("shows a non-field error once above the form", async () => {
    const onSubmit = vi.fn().mockRejectedValue(new ApiError("CONFLICT", 409, "already exists"));
    setup({ onSubmit });
    await fill("shop", cert, key);
    await userEvent.click(screen.getByRole("button", { name: "Upload" }));
    expect(await screen.findByText("already exists")).toBeInTheDocument();
  });

  it("when replacing, hides the name and lets the chain and key stay empty", async () => {
    const { onSubmit } = setup({ fixedName: "shop", optionalMaterial: true, submitLabel: "Replace" });
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ name: "shop", certPem: "", keyPem: "" }));
  });

  it("cancels", async () => {
    const { onCancel } = setup();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
  });
});
