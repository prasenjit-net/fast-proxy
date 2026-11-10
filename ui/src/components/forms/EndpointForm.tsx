import { useState, type FormEvent } from "react";
import { validateEndpoint, type FieldErrors } from "../../lib/policyValidation";
import type { Endpoint, EndpointInput } from "../../lib/types";
import { useServerErrors } from "../../lib/useServerErrors";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";

interface EndpointFormProps {
  initial?: Endpoint;
  submitLabel: string;
  onSubmit: (input: EndpointInput) => Promise<void>;
  onCancel: () => void;
}

/** One backend address of an upstream group, with its weight. */
export default function EndpointForm({ initial, submitLabel, onSubmit, onCancel }: EndpointFormProps) {
  const [addr, setAddr] = useState(initial?.addr ?? "");
  const [weight, setWeight] = useState(String(initial?.weight ?? 1));
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateEndpoint({ addr, weight });
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
      <FormField
        label="Address"
        htmlFor="endpoint-addr"
        error={error("addr")}
        hint="host:port, for example 127.0.0.1:3000 or backend.internal:8080"
      >
        <input
          id="endpoint-addr"
          className="input"
          value={addr}
          onChange={(event) => setAddr(event.target.value)}
          autoComplete="off"
          {...errorProps("endpoint-addr", error("addr"))}
        />
      </FormField>
      <FormField
        label="Weight"
        htmlFor="endpoint-weight"
        error={error("weight")}
        hint="A share of the traffic, from 1 to 1000, for the weighted algorithms."
      >
        <input
          id="endpoint-weight"
          className="input"
          inputMode="numeric"
          value={weight}
          onChange={(event) => setWeight(event.target.value)}
          {...errorProps("endpoint-weight", error("weight"))}
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
