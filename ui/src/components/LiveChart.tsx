// Live line chart of requests per second, fed by the WebSocket `traffic`
// samples. The color comes from the theme's chart token; text uses text
// tokens, never series colors. Hovering shows a crosshair + tooltip.
import { useRef, useState, type PointerEvent } from "react";
import type { Traffic } from "../lib/api";
import { formatPercent, formatRate } from "../lib/format";

const W = 640;
const H = 240;
const PAD = { top: 14, right: 14, bottom: 26, left: 44 };
const INNER_W = W - PAD.left - PAD.right;
const INNER_H = H - PAD.top - PAD.bottom;

const fmtTime = (ms: number) =>
  new Date(ms).toLocaleTimeString([], { hour12: false });

/** The smallest "nice" axis maximum (1, 2, 5, 10, 20, 50, …) that fits `value`. */
export function niceMax(value: number): number {
  if (value <= 1) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const fraction = value / magnitude;
  const nice = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  return nice * magnitude;
}

function LegendDot({ color }: { color: string }) {
  return (
    <i
      className="mr-1 inline-block size-2 rounded-full align-baseline"
      style={{ background: color }}
    />
  );
}

export default function LiveChart({ history }: { history: Traffic[] }) {
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

  const rates = history.map((sample) => sample.total.requestsPerSec);
  const top = niceMax(Math.max(...rates));
  const x = (i: number) => PAD.left + (i / (history.length - 1)) * INNER_W;
  const y = (v: number) => PAD.top + (1 - v / top) * INNER_H;
  const line = rates
    .map((v, i) => `${i === 0 ? "M" : "L"}${x(i).toFixed(1)} ${y(v).toFixed(1)}`)
    .join(" ");
  const area = `${line} L${x(history.length - 1).toFixed(1)} ${y(0)} L${x(0).toFixed(1)} ${y(0)} Z`;
  const latest = history[history.length - 1];
  const hovered = hover === null ? null : history[hover];
  const shown = hovered ?? latest;
  const timeTicks = [...new Set([0, Math.floor((history.length - 1) / 2), history.length - 1])];
  const yTicks = [0, top / 4, top / 2, (top * 3) / 4, top];

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
          <LegendDot color="var(--chart-1)" /> Requests/s{" "}
          <b className="font-semibold text-ink">{formatRate(shown.total.requestsPerSec)}</b>
        </span>
      </div>
      <svg
        ref={svgRef}
        viewBox={`0 0 ${W} ${H}`}
        className="block h-auto w-full touch-none"
        role="img"
        aria-label="Requests per second over the last few minutes"
        onPointerMove={onMove}
        onPointerLeave={() => setHover(null)}
      >
        {yTicks.map((tick) => (
          <g key={tick}>
            <line
              className="chart-grid"
              x1={PAD.left}
              x2={W - PAD.right}
              y1={y(tick)}
              y2={y(tick)}
            />
            <text className="chart-tick" x={PAD.left - 8} y={y(tick) + 3.5} textAnchor="end">
              {formatRate(tick)}
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
        <path d={area} fill="var(--chart-1)" opacity="0.1" />
        <path d={line} className="chart-line" stroke="var(--chart-1)" />
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
              cy={y(rates[hover])}
              r={4}
              fill="var(--chart-1)"
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
            left: `${Math.min(78, Math.max(2, (x(hover) / W) * 100 - 6))}%`,
          }}
        >
          <span className="text-ink-faint">{fmtTime(hovered.timestampMs)}</span>
          <span>
            <LegendDot color="var(--chart-1)" /> {formatRate(hovered.total.requestsPerSec)} req/s
          </span>
          <span>errors {formatPercent(hovered.total.errorRate)}</span>
        </div>
      ) : null}
    </div>
  );
}
