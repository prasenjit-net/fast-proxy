// Two-series live line chart fed by the WebSocket. Colors come from the
// theme's validated chart tokens (--chart-1 / --chart-2); text uses text
// tokens, never series colors. Hovering shows a crosshair + tooltip.
import { useRef, useState, type PointerEvent } from "react";
import type { Metrics } from "../lib/api";

const W = 640;
const H = 240;
const PAD = { top: 14, right: 14, bottom: 26, left: 40 };
const INNER_W = W - PAD.left - PAD.right;
const INNER_H = H - PAD.top - PAD.bottom;

const fmtTime = (ms: number) =>
  new Date(ms).toLocaleTimeString([], { hour12: false });

function LegendDot({ color }: { color: string }) {
  return (
    <i
      className="mr-1 inline-block size-2 rounded-full align-baseline"
      style={{ background: color }}
    />
  );
}

export default function LiveChart({ history }: { history: Metrics[] }) {
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [hover, setHover] = useState<number | null>(null);

  if (history.length < 2) {
    return (
      <div className="flex min-h-[200px] items-center justify-center gap-3 text-[0.88rem] text-ink-faint">
        <div className="spinner" />
        <span>Waiting for live data over the WebSocket…</span>
      </div>
    );
  }

  const x = (i: number) => PAD.left + (i / (history.length - 1)) * INNER_W;
  const y = (v: number) => PAD.top + (1 - v / 100) * INNER_H;
  const line = (pick: (m: Metrics) => number) =>
    history
      .map((m, i) => `${i === 0 ? "M" : "L"}${x(i).toFixed(1)} ${y(pick(m)).toFixed(1)}`)
      .join(" ");

  const cpuLine = line((m) => m.cpu);
  const memLine = line((m) => m.memory);
  const cpuArea = `${cpuLine} L${x(history.length - 1).toFixed(1)} ${y(0)} L${x(0).toFixed(1)} ${y(0)} Z`;
  const latest = history[history.length - 1];
  const hovered = hover === null ? null : history[hover];
  const timeTicks = [...new Set([0, Math.floor((history.length - 1) / 2), history.length - 1])];

  const onMove = (event: PointerEvent<SVGSVGElement>) => {
    const svg = svgRef.current;
    if (!svg) return;
    const rect = svg.getBoundingClientRect();
    const px = ((event.clientX - rect.left) / rect.width) * W;
    const index = Math.round(((px - PAD.left) / INNER_W) * (history.length - 1));
    setHover(Math.max(0, Math.min(history.length - 1, index)));
  };

  return (
    <div className="relative">
      <div className="mb-1.5 flex justify-end gap-5 font-mono text-[0.74rem] text-ink-muted">
        <span>
          <LegendDot color="var(--chart-1)" /> CPU{" "}
          <b className="font-semibold text-ink">{(hovered ?? latest).cpu.toFixed(0)}%</b>
        </span>
        <span>
          <LegendDot color="var(--chart-2)" /> Memory{" "}
          <b className="font-semibold text-ink">{(hovered ?? latest).memory.toFixed(0)}%</b>
        </span>
      </div>
      <svg
        ref={svgRef}
        viewBox={`0 0 ${W} ${H}`}
        className="block h-auto w-full touch-none"
        role="img"
        aria-label="CPU and memory usage over the last few minutes"
        onPointerMove={onMove}
        onPointerLeave={() => setHover(null)}
      >
        {[0, 25, 50, 75, 100].map((tick) => (
          <g key={tick}>
            <line
              className="chart-grid"
              x1={PAD.left}
              x2={W - PAD.right}
              y1={y(tick)}
              y2={y(tick)}
            />
            <text className="chart-tick" x={PAD.left - 8} y={y(tick) + 3.5} textAnchor="end">
              {tick}%
            </text>
          </g>
        ))}
        {timeTicks.map((i) => (
          <text
            key={i}
            className="chart-tick"
            x={x(i)}
            y={H - 8}
            textAnchor={i === 0 ? "start" : i === history.length - 1 ? "end" : "middle"}
          >
            {fmtTime(history[i].timestampMs)}
          </text>
        ))}
        <path d={cpuArea} fill="var(--chart-1)" opacity="0.1" />
        <path d={cpuLine} className="chart-line" stroke="var(--chart-1)" />
        <path d={memLine} className="chart-line" stroke="var(--chart-2)" />
        {hover !== null && hovered ? (
          <g>
            <line
              className="chart-crosshair"
              x1={x(hover)}
              x2={x(hover)}
              y1={PAD.top}
              y2={H - PAD.bottom}
            />
            <circle
              cx={x(hover)}
              cy={y(hovered.cpu)}
              r={4}
              fill="var(--chart-1)"
              stroke="var(--surface)"
              strokeWidth={2}
            />
            <circle
              cx={x(hover)}
              cy={y(hovered.memory)}
              r={4}
              fill="var(--chart-2)"
              stroke="var(--surface)"
              strokeWidth={2}
            />
          </g>
        ) : null}
      </svg>
      {hover !== null && hovered ? (
        <div
          className="pointer-events-none absolute top-9 z-5 flex flex-col gap-0.5 rounded-lg border border-line bg-surface px-2.5 py-2 font-mono text-[0.72rem] whitespace-nowrap text-ink-muted shadow-lg"
          style={{
            left: `${(x(hover) / W) * 100}%`,
            transform:
              x(hover) > W / 2 ? "translateX(calc(-100% - 10px))" : "translateX(10px)",
          }}
        >
          <div className="font-semibold text-ink tabular-nums">
            {fmtTime(hovered.timestampMs)}
          </div>
          <div>
            <LegendDot color="var(--chart-1)" /> CPU {hovered.cpu.toFixed(1)}%
          </div>
          <div>
            <LegendDot color="var(--chart-2)" /> Memory {hovered.memory.toFixed(1)}%
          </div>
        </div>
      ) : null}
    </div>
  );
}
