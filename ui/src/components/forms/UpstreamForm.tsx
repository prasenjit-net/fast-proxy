import { useState, type FormEvent } from "react";
import type { UpstreamInput } from "../../lib/types";
import { useServerErrors } from "../../lib/useServerErrors";
import { validateUpstream, type FieldErrors } from "../../lib/validation";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";

interface UpstreamFormProps {
  initial?: UpstreamInput;
  submitLabel: string;
  onSubmit: (input: UpstreamInput) => Promise<void>;
  onCancel: () => void;
}

/** Name, address and weight of an upstream (one endpoint per upstream in M1). */
export default function UpstreamForm({ initial, submitLabel, onSubmit, onCancel }: UpstreamFormProps) {
  const [name, setName] = useState(initial?.name ?? "");
  const [addr, setAddr] = useState(initial?.addr ?? "");
  const [weight, setWeight] = useState(String(initial?.weight ?? 1));
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateUpstream({ name, addr, weight });
    setClientErrors(result.errors);
    if (!result.value) return;
    setBusy(true);
    try {
      await onSubmit(result.value);
    } catch (thrown) {
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-4">
      <FormError message={server.general} />
      <FormField label="Name" htmlFor="upstream-name" error={error("name")}>
        <input
          id="upstream-name"
          className="input"
          value={name}
          onChange={(event) => setName(event.target.value)}
          autoComplete="off"
          {...errorProps("upstream-name", error("name"))}
        />
      </FormField>
      <FormField
        label="Address"
        htmlFor="upstream-addr"
        error={error("addr")}
        hint="host:port, for example 127.0.0.1:3000 or backend.internal:8080"
      >
        <input
          id="upstream-addr"
          className="input"
          value={addr}
          onChange={(event) => setAddr(event.target.value)}
          autoComplete="off"
          {...errorProps("upstream-addr", error("addr"))}
        />
      </FormField>
      <FormField
        label="Weight"
        htmlFor="upstream-weight"
        error={error("weight")}
        hint="Unused until load balancing arrives; leave at 1."
      >
        <input
          id="upstream-weight"
          className="input"
          inputMode="numeric"
          value={weight}
          onChange={(event) => setWeight(event.target.value)}
          {...errorProps("upstream-weight", error("weight"))}
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
