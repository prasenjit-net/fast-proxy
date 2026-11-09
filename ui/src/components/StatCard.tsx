import type { ReactElement, ReactNode } from "react";
import Sparkline from "./Sparkline";

interface StatCardProps {
  icon: ReactElement;
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  series?: number[];
  max?: number;
  color?: string;
}

export default function StatCard({
  icon,
  label,
  value,
  sub,
  series,
  max,
  color,
}: StatCardProps) {
  return (
    <div className="card flex flex-col gap-1 pb-4">
      <div className="flex items-center gap-2">
        <span className="inline-flex text-ink-faint">{icon}</span>
        <span className="font-mono text-[0.7rem] font-medium tracking-wide text-ink-muted uppercase">
          {label}
        </span>
      </div>
      <div className="text-[1.7rem] leading-tight font-semibold">{value}</div>
      {sub ? <div className="font-mono text-[0.68rem] text-ink-faint">{sub}</div> : null}
      {series && series.length > 1 ? (
        <div className="mt-2">
          <Sparkline data={series} color={color} height={36} max={max} />
        </div>
      ) : null}
    </div>
  );
}
