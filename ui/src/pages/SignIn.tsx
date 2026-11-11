import { useState, type FormEvent } from "react";
import FormError from "../components/FormError";
import FormField, { errorProps } from "../components/FormField";
import Logo from "../components/Logo";
import { ApiError } from "../lib/api";

interface SignInProps {
  /** Why the user is looking at this page again, for example a revoked token. */
  notice?: string;
  /** Verifies and stores the token; rejects with the reason it was not accepted. */
  onSignIn: (token: string) => Promise<void>;
}

/** The admin plane asks for a token before showing anything. */
export default function SignInPage({ notice, onSignIn }: SignInProps) {
  const [token, setToken] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    const trimmed = token.trim();
    if (!trimmed) {
      setError("Paste an admin token.");
      return;
    }
    setBusy(true);
    setError(undefined);
    try {
      await onSignIn(trimmed);
    } catch (thrown) {
      setError(
        thrown instanceof ApiError && thrown.status === 401
          ? "That token was not accepted. Check it, or ask an administrator for a new one."
          : thrown instanceof Error
            ? thrown.message
            : String(thrown),
      );
      setBusy(false);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center p-6">
      <form
        onSubmit={handleSubmit}
        noValidate
        className="card flex w-full max-w-[420px] flex-col gap-4"
        aria-labelledby="sign-in-title"
      >
        <div className="flex items-center gap-3">
          <Logo size={40} />
          <h1 id="sign-in-title" className="text-lg font-semibold">
            Sign in
          </h1>
        </div>
        {notice ? (
          <p role="status" className="rounded-lg bg-warn-soft px-3 py-2 text-[0.84rem] text-warn">
            {notice}
          </p>
        ) : null}
        <FormError message={error} />
        <FormField
          label="Admin token"
          htmlFor="admin-token"
          hint="It starts with fp_. The first start of the proxy prints one."
        >
          <input
            id="admin-token"
            type="password"
            className="input font-mono"
            autoComplete="off"
            autoFocus
            spellCheck={false}
            value={token}
            onChange={(event) => setToken(event.target.value)}
            {...errorProps("admin-token", undefined)}
          />
        </FormField>
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {busy ? "Checking…" : "Sign in"}
        </button>
        <p className="text-[0.76rem] leading-relaxed text-ink-faint">
          The token stays in this browser tab only and is forgotten when you sign out or close the
          tab. This page forbids inline scripts and other websites, which is what makes keeping it
          here acceptable; sign out on a shared computer.
        </p>
      </form>
    </main>
  );
}
