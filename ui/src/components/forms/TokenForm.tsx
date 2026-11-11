import { useState, type FormEvent } from "react";
import { EXPIRY_CHOICES, validateToken } from "../../lib/tokenValidation";
import type { CreatedToken, Role, TokenInput } from "../../lib/types";
import { useServerErrors } from "../../lib/useServerErrors";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";

interface TokenFormProps {
  onSubmit: (input: TokenInput) => Promise<CreatedToken>;
  onCancel: () => void;
  /** Called with the new token, once, so the page can show it. */
  onCreated: (token: CreatedToken) => void;
}

/** Name, role and expiry of a new token. */
export default function TokenForm({ onSubmit, onCancel, onCreated }: TokenFormProps) {
  const [name, setName] = useState("");
  const [role, setRole] = useState<Role>("viewer");
  const [expires, setExpires] = useState("");
  const [clientErrors, setClientErrors] = useState<Record<string, string | undefined>>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateToken({ name, role, expiresInDays: expires });
    setClientErrors(result.errors);
    if (result.value === undefined) return;
    setBusy(true);
    try {
      onCreated(await onSubmit(result.value));
    } catch (thrown) {
      server.capture(thrown);
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-4">
      <FormError message={server.general} />
      <FormField
        label="Name"
        htmlFor="token-name"
        error={error("name")}
        hint="Shown in the audit log next to everything this token does."
      >
        <input
          id="token-name"
          className="input"
          value={name}
          placeholder="ci-deploy"
          autoComplete="off"
          onChange={(event) => setName(event.target.value)}
          {...errorProps("token-name", error("name"))}
        />
      </FormField>
      <FormField
        label="Role"
        htmlFor="token-role"
        error={error("role")}
        hint={
          role === "admin"
            ? "An admin can change everything, including tokens."
            : "A viewer can read everything except key material, and change nothing."
        }
      >
        <select
          id="token-role"
          className="input"
          value={role}
          onChange={(event) => setRole(event.target.value as Role)}
        >
          <option value="viewer">viewer (read-only)</option>
          <option value="admin">admin</option>
        </select>
      </FormField>
      <FormField label="Expires" htmlFor="token-expires" error={error("expiresInDays")}>
        <select
          id="token-expires"
          className="input"
          value={expires}
          onChange={(event) => setExpires(event.target.value)}
        >
          {EXPIRY_CHOICES.map((choice) => (
            <option key={choice.value} value={choice.value}>
              {choice.label}
            </option>
          ))}
        </select>
      </FormField>
      <div className="flex justify-end gap-2">
        <button type="button" className="btn btn-secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary" disabled={busy}>
          Create token
        </button>
      </div>
    </form>
  );
}
