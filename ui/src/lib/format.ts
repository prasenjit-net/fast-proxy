export function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60) % 60;
  const h = Math.floor(secs / 3600) % 24;
  const d = Math.floor(secs / 86400);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m ${secs % 60}s`;
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value);
}

export function timeAgo(timestampMs: number): string {
  const diff = Math.max(0, Date.now() - timestampMs);
  if (diff < 10_000) return "just now";
  if (diff < 60_000) return `${Math.floor(diff / 1000)}s ago`;
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return new Date(timestampMs).toLocaleDateString();
}

/** A request rate: one decimal below 10/s, whole numbers above. */
export function formatRate(perSecond: number): string {
  return perSecond < 10 ? perSecond.toFixed(1) : formatNumber(perSecond);
}

/** A 0-1 fraction as a percentage; small non-zero values stay visible. */
export function formatPercent(fraction: number): string {
  const percent = fraction * 100;
  if (percent === 0) return "0%";
  return percent < 1 ? "<1%" : `${percent.toFixed(percent < 10 ? 1 : 0)}%`;
}

/** A duration in milliseconds as a short label: 250ms, 5s, 2m. */
export function formatDurationMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${Number((ms / 1000).toFixed(1))}s`;
  return `${Number((ms / 60_000).toFixed(1))}m`;
}

/** Time left as a clock-like label: 45s, 12:05, 1:02:03; "ending" at zero. */
export function formatCountdown(secondsLeft: number): string {
  const total = Math.max(0, Math.floor(secondsLeft));
  if (total === 0) return "ending";
  const h = Math.floor(total / 3600);
  const m = Math.floor(total / 60) % 60;
  const s = total % 60;
  const two = (n: number) => String(n).padStart(2, "0");
  if (h > 0) return `${h}:${two(m)}:${two(s)}`;
  if (m > 0) return `${m}:${two(s)}`;
  return `${s}s`;
}
