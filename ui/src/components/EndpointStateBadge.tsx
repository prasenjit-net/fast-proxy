import type { EffectiveState, EndpointStatus } from "../lib/types";
import Badge from "./Badge";

const TONES: Record<EffectiveState, "ok" | "err" | "warn" | "info" | "neutral"> = {
  healthy: "ok",
  unhealthy: "err",
  ejected: "warn",
  draining: "info",
  disabled: "neutral",
};

/**
 * What the proxy does with an endpoint right now. A draining endpoint with
 * nothing left in flight reads "drained", so an operator can see when it is
 * safe to stop.
 */
export default function EndpointStateBadge({ status }: { status: EndpointStatus }) {
  const drained = status.effective === "draining" && status.inFlight === 0;
  return (
    <span title={status.reason}>
      <Badge tone={TONES[status.effective]} dot>
        {drained ? "drained" : status.effective}
      </Badge>
    </span>
  );
}
