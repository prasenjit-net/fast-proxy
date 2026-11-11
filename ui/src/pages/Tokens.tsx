import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import Badge from "../components/Badge";
import ConfirmDialog from "../components/ConfirmDialog";
import EmptyState from "../components/EmptyState";
import TokenForm from "../components/forms/TokenForm";
import Modal from "../components/Modal";
import SecretReveal from "../components/SecretReveal";
import { useSession } from "../context/SessionContext";
import { useToast } from "../context/ToastContext";
import { IconPlus, IconTrash } from "../icons";
import { api } from "../lib/api";
import { timeAgo } from "../lib/format";
import { queryClient } from "../lib/queryClient";
import { refreshConfigData } from "../lib/refreshConfigData";
import { tokenState } from "../lib/tokenValidation";
import type { CreatedToken, Token } from "../lib/types";

const STATE_TONE = { active: "ok", revoked: "err", expired: "warn" } as const;

/** Managing tokens is for administrators; anyone else is told so. */
export default function TokensPage() {
  const { isAdmin } = useSession();
  if (!isAdmin) {
    return (
      <section className="card">
        <EmptyState title="Administrators only">
          Creating and revoking tokens needs an admin token. Ask an administrator.
        </EmptyState>
      </section>
    );
  }
  return <TokenList />;
}

function TokenList() {
  const { who } = useSession();
  const { push, notifyError } = useToast();
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CreatedToken | null>(null);
  const [revoking, setRevoking] = useState<Token | null>(null);

  const query = useQuery({ queryKey: ["tokens"], queryFn: api.tokens });

  const revoke = useMutation({
    mutationFn: (token: Token) => api.revokeToken(token.id),
    onSuccess: async (_, token) => {
      await refreshConfigData(queryClient);
      push("success", `Revoked ${token.name}`);
      setRevoking(null);
    },
    onError: (error) => {
      setRevoking(null);
      notifyError(error); // for example the 409 for the last admin token
    },
  });

  const now = Date.now();
  let body;
  if (query.isError) {
    body = <p className="text-[0.9rem] text-err">Could not load tokens: {query.error.message}</p>;
  } else if (!query.data) {
    body = (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  } else if (query.data.data.length === 0) {
    body = (
      <EmptyState title="No tokens">
        Create a token for each person or tool that needs the admin API.
      </EmptyState>
    );
  } else {
    body = (
      <div className="overflow-x-auto">
        <table className="w-full text-left text-[0.88rem]">
          <thead>
            <tr className="border-b border-line text-[0.74rem] tracking-wide text-ink-faint uppercase">
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Role</th>
              <th className="py-2 pr-4 font-medium">State</th>
              <th className="py-2 pr-4 font-medium">Created</th>
              <th className="py-2 pr-4 font-medium">Expires</th>
              <th className="py-2 pr-4 font-medium">Last used</th>
              <th className="py-2 font-medium" />
            </tr>
          </thead>
          <tbody>
            {query.data.data.map((token) => {
              const state = tokenState(token, now);
              return (
                <tr key={token.id} className="border-b border-line last:border-b-0">
                  <td className="py-2.5 pr-4 font-medium">
                    {token.name}
                    {token.name === who.name ? (
                      <span className="ml-2 text-[0.74rem] text-ink-faint">(you)</span>
                    ) : null}
                  </td>
                  <td className="py-2.5 pr-4">
                    <Badge tone={token.role === "admin" ? "accent" : "info"}>{token.role}</Badge>
                  </td>
                  <td className="py-2.5 pr-4">
                    <Badge tone={STATE_TONE[state]} dot>
                      {state}
                    </Badge>
                  </td>
                  <td
                    className="py-2.5 pr-4 text-ink-faint"
                    title={new Date(token.createdAtMs).toLocaleString()}
                  >
                    {timeAgo(token.createdAtMs)} by {token.createdBy}
                  </td>
                  <td className="py-2.5 pr-4 text-ink-faint">
                    {token.expiresAtMs === null
                      ? "never"
                      : new Date(token.expiresAtMs).toLocaleDateString()}
                  </td>
                  <td className="py-2.5 pr-4 text-ink-faint">
                    {token.lastUsedAtMs === null ? "never" : timeAgo(token.lastUsedAtMs)}
                  </td>
                  <td className="py-2.5 text-right">
                    {token.revokedAtMs === null ? (
                      <button
                        className="icon-btn danger"
                        aria-label={`Revoke ${token.name}`}
                        onClick={() => setRevoking(token)}
                      >
                        <IconTrash size={18} />
                      </button>
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    );
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Tokens</h2>
        <span className="card-hint">only a hash is stored; a secret is shown once, when created</span>
        <button className="btn btn-primary btn-sm ml-auto" onClick={() => setCreating(true)}>
          <IconPlus size={16} /> New token
        </button>
      </div>
      {body}
      {creating ? (
        <Modal title="New token" onClose={() => setCreating(false)}>
          <TokenForm
            onSubmit={async (input) => (await api.createToken(input)).data}
            onCancel={() => setCreating(false)}
            onCreated={(token) => {
              setCreating(false);
              setCreated(token);
              void refreshConfigData(queryClient);
            }}
          />
        </Modal>
      ) : null}
      {created ? (
        <Modal title={`Token ${created.name} created`} onClose={() => setCreated(null)}>
          <SecretReveal secret={created.secret} label="token" />
          <div className="mt-4 flex justify-end">
            <button className="btn btn-primary" onClick={() => setCreated(null)}>
              Done
            </button>
          </div>
        </Modal>
      ) : null}
      {revoking ? (
        <ConfirmDialog
          title={`Revoke ${revoking.name}?`}
          confirmLabel="Revoke"
          busy={revoke.isPending}
          onCancel={() => setRevoking(null)}
          onConfirm={() => revoke.mutate(revoking)}
        >
          <p>
            Anything using this token is locked out at once, on every instance sharing this
            database. This cannot be undone.
          </p>
          {revoking.name === who.name ? (
            <p className="mt-2 font-medium text-warn">
              This is the token you are signed in with: you will be signed out.
            </p>
          ) : null}
        </ConfirmDialog>
      ) : null}
    </section>
  );
}
