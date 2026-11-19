// Brand mark: one stream in, a routing hub, routes out and the chosen one
// forward. The inline sibling of public/favicon.svg (and docs/assets/logo.svg),
// drawn with theme tokens so it adapts to light and dark automatically.
export default function Logo({ size = 32 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
      <rect width="64" height="64" rx="14" fill="var(--accent)" />
      <g fill="none" stroke="var(--on-accent)" strokeWidth="4.5" strokeLinecap="round">
        <path d="M9 32h11" />
        <g opacity="0.55">
          <path d="M20 32c11 0 10-15 21-15h4" />
          <path d="M20 32c11 0 10 15 21 15h4" />
        </g>
        <path d="M20 32h30" />
        <path d="M45 25.5l7 6.5-7 6.5" strokeLinejoin="round" />
      </g>
      <g fill="var(--on-accent)" opacity="0.55">
        <circle cx="49" cy="17" r="4" />
        <circle cx="49" cy="47" r="4" />
      </g>
      <circle cx="20" cy="32" r="6.5" fill="var(--on-accent)" />
      <circle cx="20" cy="32" r="2.6" fill="var(--accent)" />
    </svg>
  );
}
