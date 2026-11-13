// A subsystem's or a route's sampling override: a badge that says what it
// changes, and (for an admin) a dialog to set or clear it.
import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useCanWrite } from "../../context/SessionContext";
import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import {
  isInherited,
  overrideSummary,
  overrideValuesOf,
  validateOverride,
  type OverrideValues,
  type Tri,
} from "../../lib/samplingValidation";
import { SAMPLING_KEY, overrideOf, useSampling } from "../../lib/useSampling";
import { useServerErrors } from "../../lib/useServerErrors";
import type { SamplingOverride } from "../../lib/types";
import Badge from "../Badge";
import FormError from "../FormError";
import FormField, { errorProps } from "../FormField";
import Modal from "../Modal";

type Target = { subsystemId: number } | { routeId: number };

const TRI: { value: Tri; label: string }[] = [
  { value: "inherit", label: "same as the global setting" },
  { value: "on", label: "on" },
  { value: "off", label: "off" },
];

function TriSelect(props: {
  id: string;
  label: string;
  value: Tri;
  onChange: (value: Tri) => void;
  error?: string;
}) {
  return (
    <FormField label={props.label} htmlFor={props.id} error={props.error}>
      <select
        id={props.id}
        className="input"
        value={props.value}
        onChange={(event) => props.onChange(event.target.value as Tri)}
      >
        {TRI.map((choice) => (
          <option key={choice.value} value={choice.value}>
            {choice.label}
          </option>
        ))}
      </select>
    </FormField>
  );
}

function OverrideDialog(props: {
  title: string;
  target: Target;
  current: SamplingOverride | undefined;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const { push, notifyError } = useToast();
  const [values, setValues] = useState<OverrideValues>(() => overrideValuesOf(props.current));
  const [clientErrors, setClientErrors] = useState<Record<string, string | undefined>>({});
  const [busy, setBusy] = useState(false);
  const server = useServerErrors();
  const error = (field: string) => clientErrors[field] ?? server.field(field);

  async function save(event: FormEvent) {
    event.preventDefault();
    server.clear();
    const result = validateOverride(values, props.target);
    setClientErrors(result.errors);
    if (result.value === undefined) return;
    setBusy(true);
    try {
      if (isInherited(values)) {
        await api.deleteSamplingOverride(props.target);
      } else {
        await api.putSamplingOverride(result.value);
      }
      await queryClient.invalidateQueries({ queryKey: [SAMPLING_KEY] });
      push("success", "Sampling override saved");
      props.onClose();
    } catch (thrown) {
      try {
        server.capture(thrown);
      } catch {
        notifyError(thrown);
      }
      setBusy(false);
    }
  }

  return (
    <Modal title={props.title} onClose={props.onClose}>
      <form onSubmit={save} noValidate className="flex flex-col gap-4">
        <FormError message={server.general} />
        <TriSelect
          id="override-enabled"
          label="Sampling"
          value={values.enabled}
          onChange={(enabled) => setValues({ ...values, enabled })}
          error={error("enabled")}
        />
        <TriSelect
          id="override-bodies"
          label="Keep request and response bodies"
          value={values.captureBodies}
          onChange={(captureBodies) => setValues({ ...values, captureBodies })}
          error={error("captureBodies")}
        />
        {values.captureBodies === "on" ? (
          <p role="note" className="rounded-md bg-warn-soft p-3 text-[0.84rem] text-warn">
            Bodies can hold personal data. Known secrets are redacted, but only what the redaction
            lists name. Turn this on only where you need it, and off again afterwards.
          </p>
        ) : null}
        <FormField
          label="Baseline (per thousand)"
          htmlFor="override-baseline"
          error={error("baselinePerMille")}
          hint="Empty keeps the global rate."
        >
          <input
            id="override-baseline"
            className="input"
            inputMode="numeric"
            value={values.baselinePerMille}
            onChange={(event) => setValues({ ...values, baselinePerMille: event.target.value })}
            {...errorProps("override-baseline", error("baselinePerMille"))}
          />
        </FormField>
        <FormField
          label="Slow after (ms)"
          htmlFor="override-slow"
          error={error("slowThresholdMs")}
          hint="Empty keeps the global threshold."
        >
          <input
            id="override-slow"
            className="input"
            inputMode="numeric"
            value={values.slowThresholdMs}
            onChange={(event) => setValues({ ...values, slowThresholdMs: event.target.value })}
            {...errorProps("override-slow", error("slowThresholdMs"))}
          />
        </FormField>
        <div className="flex justify-end gap-2">
          <button type="button" className="btn btn-secondary" onClick={props.onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            {isInherited(values) ? "Clear override" : "Save override"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

/** What sampling does for `target`, and for an admin a way to change it. */
export default function SamplingControl({ target, label }: { target: Target; label: string }) {
  const canWrite = useCanWrite();
  const sampling = useSampling();
  const [editing, setEditing] = useState(false);
  const current = overrideOf(sampling.data?.data, target);
  if (!sampling.data) return null;
  return (
    <span className="inline-flex items-center gap-2">
      <Badge tone={current ? "accent" : "neutral"}>sampling: {overrideSummary(current)}</Badge>
      {canWrite ? (
        <button
          className="btn btn-ghost btn-sm"
          aria-label={`Edit sampling for ${label}`}
          onClick={() => setEditing(true)}
        >
          Sampling
        </button>
      ) : null}
      {editing ? (
        <OverrideDialog
          title={`Sampling for ${label}`}
          target={target}
          current={current}
          onClose={() => setEditing(false)}
        />
      ) : null}
    </span>
  );
}
