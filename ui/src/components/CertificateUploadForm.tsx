import { useState, type ChangeEvent, type FormEvent } from "react";
import { readFileText } from "../lib/certificates";
import type { CertificateUpload } from "../lib/types";
import { useServerErrors } from "../lib/useServerErrors";
import { validateCertificateUpload } from "../lib/tlsValidation";
import type { FieldErrors } from "../lib/validation";
import FormError from "./FormError";
import FormField, { errorProps } from "./FormField";

interface Props {
  submitLabel: string;
  /** Fixed name (for example when replacing); the name field is hidden. */
  fixedName?: string;
  /** Replacing: the chain and key may each be left empty to keep the stored one. */
  optionalMaterial?: boolean;
  onSubmit: (upload: CertificateUpload) => Promise<void>;
  onCancel: () => void;
}

/** Name, certificate chain and private key. Pasted or chosen from a file read in the browser. */
export default function CertificateUploadForm({
  submitLabel,
  fixedName,
  optionalMaterial = false,
  onSubmit,
  onCancel,
}: Props) {
  const [name, setName] = useState(fixedName ?? "");
  const [certPem, setCertPem] = useState("");
  const [keyPem, setKeyPem] = useState("");
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function chooseFile(event: ChangeEvent<HTMLInputElement>, set: (text: string) => void) {
    const file = event.target.files?.[0];
    if (file) set(await readFileText(file));
    event.target.value = "";
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const values = { name: fixedName ?? name, certPem, keyPem };
    // When replacing, an empty field keeps the stored value and is not checked.
    const result = optionalMaterial
      ? { errors: {} as FieldErrors, value: values }
      : validateCertificateUpload(values);
    setClientErrors(result.errors);
    if (!result.value) return;
    setBusy(true);
    try {
      await onSubmit(result.value);
      // The key is write-only: it is gone from the form as soon as it was sent.
      setKeyPem("");
    } catch (thrown) {
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }

  const materialHint = optionalMaterial ? "Leave empty to keep the stored one." : undefined;

  return (
    <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-4" autoComplete="off">
      <FormError message={server.general} />
      {fixedName === undefined ? (
        <FormField
          label="Name"
          htmlFor="cert-name"
          error={error("name")}
          hint="Letters, digits, '.', '_' and '-'. Shown in metrics and status."
        >
          <input
            id="cert-name"
            className="input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            {...errorProps("cert-name", error("name"))}
          />
        </FormField>
      ) : null}
      <FormField
        label="Certificate chain (PEM)"
        htmlFor="cert-pem"
        error={error("certPem")}
        hint={materialHint ?? "The server certificate first, then any intermediates."}
      >
        <textarea
          id="cert-pem"
          className="input min-h-[110px] font-mono text-[0.78rem]"
          value={certPem}
          onChange={(event) => setCertPem(event.target.value)}
          spellCheck={false}
          {...errorProps("cert-pem", error("certPem"))}
        />
        <input
          type="file"
          aria-label="Choose a certificate file"
          className="text-[0.8rem]"
          onChange={(event) => void chooseFile(event, setCertPem)}
        />
      </FormField>
      <FormField
        label="Private key (PEM)"
        htmlFor="key-pem"
        error={error("keyPem")}
        hint={
          materialHint ??
          "Unencrypted. It is encrypted at rest, never shown again, and never leaves this proxy."
        }
      >
        <textarea
          id="key-pem"
          className="input min-h-[90px] font-mono text-[0.78rem]"
          value={keyPem}
          onChange={(event) => setKeyPem(event.target.value)}
          spellCheck={false}
          autoComplete="off"
          {...errorProps("key-pem", error("keyPem"))}
        />
        <input
          type="file"
          aria-label="Choose a private key file"
          className="text-[0.8rem]"
          onChange={(event) => void chooseFile(event, setKeyPem)}
        />
      </FormField>
      <div className="flex justify-end gap-2">
        <button type="button" className="btn btn-secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}
