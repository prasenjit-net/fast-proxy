import { formatCountdown } from "../../lib/format";
import { useNow } from "../../lib/useNow";

/** The time left before a scope ends by itself, ticking each second; null means until turned off. */
export default function Countdown({ expiresAtMs }: { expiresAtMs: number | null }) {
  const now = useNow();
  if (expiresAtMs === null) return <span>until turned off</span>;
  const left = formatCountdown((expiresAtMs - now) / 1000);
  return (
    <span title={new Date(expiresAtMs).toLocaleString()}>
      {left === "ending" ? "ending" : `ends in ${left}`}
    </span>
  );
}
