import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import { queryClient } from "../../lib/queryClient";
import { refreshConfigData } from "../../lib/refreshConfigData";
import type { MaintenanceScope, MaintenanceTarget } from "../../lib/types";
import { isScopeOf, stillActive, useMaintenance } from "../../lib/useMaintenance";
import { useNow } from "../../lib/useNow";
import Badge from "../Badge";
import ConfirmDialog from "../ConfirmDialog";
import Modal from "../Modal";
import SecretReveal from "../SecretReveal";
import WriteOnly from "../WriteOnly";
import Countdown from "./Countdown";
import MaintenanceForm from "./MaintenanceForm";

/** What the scope covers, for buttons and headings. */
export function coversOf(target: MaintenanceTarget): string {
  switch (target.scope) {
    case "global":
      return "the whole proxy";
    case "subsystem":
      return `subsystem ${target.name}`;
    case "route":
      return `route ${target.prefix}`;
  }
}

/** What opens again when the scope is turned off, in plain words. */
function reopens(target: MaintenanceTarget): string {
  switch (target.scope) {
    case "global":
      return "Every host on this proxy answers normally again, except subsystems and routes that are in maintenance themselves.";
    case "subsystem":
      return "Its hosts answer normally again, except routes that are in maintenance themselves. Maintenance on the whole proxy, if any, still applies.";
    case "route":
      return "Requests under that path reach their upstream again. Maintenance on its subsystem or the whole proxy, if any, still applies.";
  }
}

/** The scope this target names, while it is on, from the shared list. */
export function useActiveScope(target: MaintenanceTarget): {
  scope: MaintenanceScope | undefined;
  loaded: boolean;
} {
  const query = useMaintenance();
  const now = useNow();
  const scope = query.data?.data.find((item) => isScopeOf(item, target) && stillActive(item, now));
  return { scope, loaded: query.data !== undefined };
}

/**
 * The state of one scope and, for an administrator, the buttons to turn
 * maintenance on or off. Everyone sees the state and the countdown.
 */
export default function MaintenanceControl({ target }: { target: MaintenanceTarget }) {
  const { scope } = useActiveScope(target);
  const { push, notifyError } = useToast();
  const [editing, setEditing] = useState(false);
  const [secret, setSecret] = useState<string | null>(null);
  const [confirmingOff, setConfirmingOff] = useState(false);
  const covers = coversOf(target);

  const turnOff = useMutation({
    mutationFn: () => api.clearMaintenance(target),
    onSuccess: async () => {
      await refreshConfigData(queryClient);
      push("success", `Maintenance is off for ${covers}`);
      setConfirmingOff(false);
    },
    onError: (error) => {
      setConfirmingOff(false);
      notifyError(error);
    },
  });

  return (
    <div className="flex flex-wrap items-center gap-3">
      {scope ? (
        <>
          <Badge tone="warn" dot>
            in maintenance
          </Badge>
          <span className="text-[0.84rem] text-ink-muted">
            <Countdown expiresAtMs={scope.expiresAtMs} />
            {scope.message ? <> · “{scope.message}”</> : null}
          </span>
        </>
      ) : (
        <Badge tone="ok" dot>
          open
        </Badge>
      )}
      <WriteOnly>
        <span className="ml-auto flex gap-2">
          <button className="btn btn-secondary btn-sm" onClick={() => setEditing(true)}>
            {scope ? "Change" : "Turn on"}
            <span className="sr-only"> maintenance for {covers}</span>
          </button>
          {scope ? (
            <button className="btn btn-danger btn-sm" onClick={() => setConfirmingOff(true)}>
              Turn off<span className="sr-only"> maintenance for {covers}</span>
            </button>
          ) : null}
        </span>
      </WriteOnly>

      {editing ? (
        <Modal wide title={`Maintenance for ${covers}`} onClose={() => setEditing(false)}>
          <MaintenanceForm
            covers={covers}
            onSubmit={(input) => api.setMaintenance(target, input)}
            onCancel={() => setEditing(false)}
            onDone={(result) => {
              setEditing(false);
              setSecret(result.bypassSecret);
              void refreshConfigData(queryClient);
              push("success", `Maintenance is on for ${covers}`);
            }}
          />
        </Modal>
      ) : null}
      {secret ? (
        <Modal title="Bypass secret" onClose={() => setSecret(null)}>
          <SecretReveal secret={secret} label="bypass secret" />
          <div className="mt-4 flex justify-end">
            <button className="btn btn-primary" onClick={() => setSecret(null)}>
              Done
            </button>
          </div>
        </Modal>
      ) : null}
      {confirmingOff ? (
        <ConfirmDialog
          title={`Turn off maintenance for ${covers}?`}
          confirmLabel="Turn off"
          busy={turnOff.isPending}
          onCancel={() => setConfirmingOff(false)}
          onConfirm={() => turnOff.mutate()}
        >
          <p>{reopens(target)}</p>
        </ConfirmDialog>
      ) : null}
    </div>
  );
}
