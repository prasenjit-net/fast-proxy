// Brand mark — the inline sibling of public/favicon.svg, drawn with
// theme tokens so it adapts to light/dark automatically.
export default function Logo({ size = 32 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
      <rect width="64" height="64" rx="14" fill="var(--accent)" />
      <circle
        cx="32"
        cy="32"
        r="14"
        fill="none"
        stroke="var(--on-accent)"
        strokeWidth="3.6"
        opacity="0.45"
      />
      <path d="M35.5 18 24 35h7l-2.5 11L40 29h-7l2.5-11z" fill="var(--on-accent)" />
    </svg>
  );
}
