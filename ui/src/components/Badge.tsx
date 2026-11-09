import type { ReactNode } from "react";

type Tone = "neutral" | "accent" | "ok" | "warn" | "err" | "info";

const TONES: Record<Tone, string> = {
  neutral: "bg-surface-2 text-ink-muted",
  accent: "bg-accent-soft text-accent",
  ok: "bg-ok-soft text-ok",
  warn: "bg-warn-soft text-warn",
  err: "bg-err-soft text-err",
  info: "bg-info-soft text-info",
};

interface BadgeProps {
  tone?: Tone;
  dot?: boolean;
  children: ReactNode;
}

export default function Badge({ tone = "neutral", dot = false, children }: BadgeProps) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 font-mono text-[0.72rem] font-medium ${TONES[tone]}`}
    >
      {dot ? <i className="size-1.5 rounded-full bg-current" /> : null}
      {children}
    </span>
  );
}
