interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label?: string;
  disabled?: boolean;
}

export default function Toggle({ checked, onChange, label, disabled }: ToggleProps) {
  return (
    <label
      className={`inline-flex items-center gap-2 select-none ${
        disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"
      }`}
    >
      <input
        type="checkbox"
        className="peer sr-only"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span
        className={`flex h-5 w-9 shrink-0 rounded-full p-[3px] transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent ${
          checked ? "bg-accent" : "bg-line-strong"
        }`}
      >
        <span
          className={`size-3.5 rounded-full bg-surface transition-transform ${
            checked ? "translate-x-4" : ""
          }`}
        />
      </span>
      {label ? <span className="text-[0.88rem]">{label}</span> : null}
    </label>
  );
}
