// Minimal inline trend line for stat tiles: no axes, no labels — the
// tile's value carries the number, the sparkline carries the shape.
interface SparklineProps {
  data: number[];
  color?: string;
  height?: number;
  /** Fixed scale maximum (e.g. 100 for percentages); defaults to the data max. */
  max?: number;
}

export default function Sparkline({
  data,
  color = "var(--chart-1)",
  height = 36,
  max,
}: SparklineProps) {
  const width = 100;
  const top = max ?? Math.max(...data, 1);
  const stepX = width / (data.length - 1);
  const points = data.map(
    (value, i) =>
      [i * stepX, height - 3 - (Math.min(value, top) / top) * (height - 6)] as const,
  );
  const line = points
    .map(([x, y], i) => `${i === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`)
    .join(" ");
  const area = `${line} L${width} ${height} L0 ${height} Z`;
  return (
    <svg
      className="block h-9 w-full"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <path d={area} fill={color} opacity={0.1} />
      <path
        d={line}
        fill="none"
        stroke={color}
        strokeWidth={2}
        strokeLinejoin="round"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
