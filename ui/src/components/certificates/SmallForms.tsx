import { useState, type FormEvent } from "react";
import { useServerErrors } from "../../lib/useServerErrors";
import { validateCaBundle, validateMaterialName, validateSelfSigned } from "../../lib/tlsValidation";
import type { FieldErrors } from "../../lib/validation";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";

interface FormShellProps {
  submitLabel: string;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (event: FormEvent) => void;
  general: string | undefined;
  children: React.ReactNode;
}

function FormShell({ submitLabel, busy, onCancel, onSubmit, general, children }: FormShellProps) {
  return (
    <form onSubmit={onSubmit} noValidate className="flex flex-col gap-4">
      <FormError message={general} />
      {children}
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

/** Runs `action` with the busy flag and server errors handled the same way everywhere. */
function useSubmit() {
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function run(errors: FieldErrors, action: (() => Promise<void>) | null) {
    server.clear();
    setClientErrors(errors);
    if (!action) return;
    setBusy(true);
    try {
      await action();
    } catch (thrown) {
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }
  return { busy, server, error, run };
}

/** Generates a self-signed certificate for some host names. */
export function SelfSignedForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (name: string, names: string[]) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [names, setNames] = useState("");
  const { busy, server, error, run } = useSubmit();

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    const checked = validateSelfSigned({ name, names });
    const value = checked.value;
    void run(checked.errors, value ? () => onSubmit(value.name, value.names) : null);
  }

  return (
    <FormShell submitLabel="Generate" busy={busy} onCancel={onCancel} onSubmit={handleSubmit} general={server.general}>
      <FormField label="Name" htmlFor="selfsigned-name" error={error("name")}>
        <input
          id="selfsigned-name"
          className="input"
          value={name}
          onChange={(event) => setName(event.target.value)}
          {...errorProps("selfsigned-name", error("name"))}
        />
      </FormField>
      <FormField
        label="Host names"
        htmlFor="selfsigned-names"
        error={error("names")}
        hint="Separated by commas or spaces. Wildcards such as *.example.com are fine."
      >
        <input
          id="selfsigned-names"
          className="input"
          value={names}
          onChange={(event) => setNames(event.target.value)}
          {...errorProps("selfsigned-names", error("names"))}
        />
      </FormField>
      <p className="text-[0.8rem] text-ink-faint">
        A self-signed certificate is for development: browsers do not trust it.
      </p>
    </FormShell>
  );
}

/** Renames a certificate. */
export function RenameForm({
  current,
  onSubmit,
  onCancel,
}: {
  current: string;
  onSubmit: (name: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(current);
  const { busy, server, error, run } = useSubmit();

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    const problem = validateMaterialName(name);
    void run(problem ? { name: problem } : {}, problem ? null : () => onSubmit(name.trim()));
  }

  return (
    <FormShell submitLabel="Rename" busy={busy} onCancel={onCancel} onSubmit={handleSubmit} general={server.general}>
      <FormField label="Name" htmlFor="rename-name" error={error("name")}>
        <input
          id="rename-name"
          className="input"
          value={name}
          onChange={(event) => setName(event.target.value)}
          {...errorProps("rename-name", error("name"))}
        />
      </FormField>
    </FormShell>
  );
}

/** Uploads a CA bundle: a name and the PEM of one or more CA certificates. */
export function CaBundleForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (name: string, pem: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [pem, setPem] = useState("");
  const { busy, server, error, run } = useSubmit();

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    const checked = validateCaBundle({ name, pem });
    const value = checked.value;
    void run(checked.errors, value ? () => onSubmit(value.name, value.pem) : null);
  }

  return (
    <FormShell submitLabel="Upload bundle" busy={busy} onCancel={onCancel} onSubmit={handleSubmit} general={server.general}>
      <FormField label="Name" htmlFor="bundle-name" error={error("name")} hint="Groups choose a bundle by this name.">
        <input
          id="bundle-name"
          className="input"
          value={name}
          onChange={(event) => setName(event.target.value)}
          {...errorProps("bundle-name", error("name"))}
        />
      </FormField>
      <FormField label="CA certificates (PEM)" htmlFor="bundle-pem" error={error("pem")}>
        <textarea
          id="bundle-pem"
          className="input min-h-[120px] font-mono text-[0.78rem]"
          value={pem}
          onChange={(event) => setPem(event.target.value)}
          spellCheck={false}
          {...errorProps("bundle-pem", error("pem"))}
        />
      </FormField>
    </FormShell>
  );
}
