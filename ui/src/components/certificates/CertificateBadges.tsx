import { SOURCE_LABELS, stateTone } from "../../lib/certificates";
import type { CertificateSource, CertificateState } from "../../lib/types";
import Badge from "../Badge";

/** Where a certificate came from; a self-signed one is called out as a warning. */
export function SourceBadge({ source }: { source: CertificateSource }) {
  return <Badge tone={source === "self_signed" ? "warn" : "neutral"}>{SOURCE_LABELS[source]}</Badge>;
}

export function StateBadge({ state }: { state: CertificateState }) {
  return (
    <Badge tone={stateTone(state)} dot>
      {state}
    </Badge>
  );
}
