import { useLive, type ConnectionStatus } from "../context/LiveContext";
import { IconWifi, IconWifiOff } from "../icons";

const LABELS: Record<ConnectionStatus, string> = {
  online: "Live",
  connecting: "Connecting",
  offline: "Offline",
};

const TONES: Record<ConnectionStatus, string> = {
  online: "bg-ok-soft text-ok",
  connecting: "bg-warn-soft text-warn",
  offline: "bg-err-soft text-err",
};

/** Topbar indicator for the server-push WebSocket. */
export default function ConnectionBadge() {
  const { status } = useLive();
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 font-mono text-[0.72rem] font-semibold ${TONES[status]}`}
      title="WebSocket connection status"
    >
      {status === "offline" ? <IconWifiOff size={14} /> : <IconWifi size={14} />}
      <span className="max-sm:hidden">{LABELS[status]}</span>
    </span>
  );
}
