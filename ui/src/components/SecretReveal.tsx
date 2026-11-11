import { useState } from "react";

interface SecretRevealProps {
  /** The secret, shown in full. */
  secret: string;
  /** What it is, for the field's label. */
  label: string;
}

/** A secret shown in a copyable box, with the warning that it is shown once. */
export default function SecretReveal({ secret, label }: SecretRevealProps) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
    } catch {
      // No clipboard permission: the text is selected for a manual copy.
      document.getElementById("revealed-secret")?.focus();
      (document.getElementById("revealed-secret") as HTMLInputElement | null)?.select();
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <p role="alert" className="rounded-lg bg-warn-soft px-3 py-2 text-[0.84rem] text-warn">
        This is the only time the {label} is shown. Copy it now and keep it somewhere safe: it
        cannot be shown again, only replaced.
      </p>
      <div className="flex gap-2">
        <input
          id="revealed-secret"
          aria-label={label}
          readOnly
          className="input font-mono text-[0.82rem]"
          value={secret}
          onFocus={(event) => event.currentTarget.select()}
        />
        <button type="button" className="btn btn-secondary" onClick={() => void copy()}>
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
    </div>
  );
}
