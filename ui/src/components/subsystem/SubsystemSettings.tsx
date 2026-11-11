import { useCanWrite } from "../../context/SessionContext";
import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { SubsystemDetail, SubsystemInput } from "../../lib/types";
import SubsystemForm from "../forms/SubsystemForm";

/** Rename, describe, or switch a subsystem on and off. */
export default function SubsystemSettings({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push } = useToast();
  const canWrite = useCanWrite();

  async function save(input: SubsystemInput) {
    await api.updateSubsystem(subsystem.id, input);
    await refresh();
    push("success", `Saved ${input.name}`);
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Settings</h2>
        <span className="card-hint">a disabled subsystem answers 503</span>
      </div>
      {canWrite ? null : (
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-[0.88rem]">
          <dt className="text-ink-muted">Name</dt>
          <dd>{subsystem.name}</dd>
          <dt className="text-ink-muted">Description</dt>
          <dd>{subsystem.description || "—"}</dd>
          <dt className="text-ink-muted">Enabled</dt>
          <dd>{subsystem.enabled ? "yes" : "no"}</dd>
        </dl>
      )}
      {/* Remount when the saved values change so the form shows the new ones. */}
      {canWrite ? <SubsystemForm
        key={`${subsystem.name}|${subsystem.description}|${subsystem.enabled}`}
        initial={{
          name: subsystem.name,
          description: subsystem.description,
          enabled: subsystem.enabled,
        }}
        submitLabel="Save settings"
        onSubmit={save}
      /> : null}
    </section>
  );
}
