// A labelled form control with an optional hint and an inline error. The
// control inside must use the same `htmlFor` as its `id`.
import type { ReactNode } from "react";

interface FormFieldProps {
  label: string;
  htmlFor: string;
  children: ReactNode;
  hint?: string;
  error?: string;
}

export default function FormField({ label, htmlFor, children, hint, error }: FormFieldProps) {
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={htmlFor} className="text-[0.82rem] font-medium">
        {label}
      </label>
      {children}
      {error ? (
        <p id={`${htmlFor}-error`} role="alert" className="text-[0.78rem] text-err">
          {error}
        </p>
      ) : hint ? (
        <p className="text-[0.76rem] text-ink-faint">{hint}</p>
      ) : null}
    </div>
  );
}

/** Props that tie an input to its error message, for accessibility. */
export function errorProps(id: string, error: string | undefined) {
  return error ? { "aria-invalid": true, "aria-describedby": `${id}-error` } : {};
}
