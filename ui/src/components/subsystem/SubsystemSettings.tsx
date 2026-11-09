import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { SubsystemDetail, SubsystemInput } from "../../lib/types";
import SubsystemForm from "../forms/SubsystemForm";

/** Rename, describe, or switch a subsystem on and off. */
export default function SubsystemSettings({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push } = useToast();

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
      {/* Remount when the saved values change so the form shows the new ones. */}
      <SubsystemForm
        key={`${subsystem.name}|${subsystem.description}|${subsystem.enabled}`}
        initial={{
          name: subsystem.name,
          description: subsystem.description,
          enabled: subsystem.enabled,
        }}
        submitLabel="Save settings"
        onSubmit={save}
      />
    </section>
  );
}
