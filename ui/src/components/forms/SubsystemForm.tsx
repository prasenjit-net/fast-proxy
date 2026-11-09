import { useState, type FormEvent } from "react";
import type { SubsystemInput } from "../../lib/types";
import { useServerErrors } from "../../lib/useServerErrors";
import { validateSubsystem, type FieldErrors } from "../../lib/validation";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";
import Toggle from "../Toggle";

interface SubsystemFormProps {
  initial?: SubsystemInput;
  submitLabel: string;
  onSubmit: (input: SubsystemInput) => Promise<void>;
  onCancel?: () => void;
}

/** Name, description and enabled switch of a subsystem. */
export default function SubsystemForm({ initial, submitLabel, onSubmit, onCancel }: SubsystemFormProps) {
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateSubsystem({ name, description, enabled });
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
        label="Name"
        htmlFor="subsystem-name"
        error={error("name")}
        hint="Lowercase letters, digits and '-'. Used in URLs and metric labels."
      >
        <input
          id="subsystem-name"
          className="input"
          value={name}
          onChange={(event) => setName(event.target.value)}
          autoComplete="off"
          {...errorProps("subsystem-name", error("name"))}
        />
      </FormField>
      <FormField label="Description" htmlFor="subsystem-description" error={error("description")}>
        <textarea
          id="subsystem-description"
          className="input min-h-[72px]"
          value={description}
          onChange={(event) => setDescription(event.target.value)}
          {...errorProps("subsystem-description", error("description"))}
        />
      </FormField>
      <Toggle checked={enabled} onChange={setEnabled} label="Enabled (serving traffic)" />
      <div className="flex justify-end gap-2">
        {onCancel ? (
          <button type="button" className="btn btn-secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
        ) : null}
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}
