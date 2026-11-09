import { useState, type FormEvent } from "react";
import { useServerErrors } from "../../lib/useServerErrors";
import { validateHost } from "../../lib/validation";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";

interface HostFormProps {
  onSubmit: (pattern: string) => Promise<void>;
}

/** One input to add a host pattern to a subsystem. */
export default function HostForm({ onSubmit }: HostFormProps) {
  const [pattern, setPattern] = useState("");
  const [clientError, setClientError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = clientError ?? server.field("pattern");

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateHost(pattern);
    setClientError(result.errors.pattern);
    if (result.value === undefined) return;
    setBusy(true);
    try {
      await onSubmit(result.value);
      setPattern("");
    } catch (thrown) {
      server.capture(thrown);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-2">
      <FormError message={server.general} />
      <div className="flex items-start gap-2">
        <div className="flex-1">
          <FormField
            label="Add a host"
            htmlFor="host-pattern"
            error={error}
            hint="An exact name, or *.example.com for any subdomain."
          >
            <input
              id="host-pattern"
              className="input"
              value={pattern}
              placeholder="shop.example.com"
              onChange={(event) => setPattern(event.target.value)}
              autoComplete="off"
              {...errorProps("host-pattern", error)}
            />
          </FormField>
        </div>
        <button type="submit" className="btn btn-secondary mt-[1.55rem]" disabled={busy}>
          Add host
        </button>
      </div>
    </form>
  );
}
