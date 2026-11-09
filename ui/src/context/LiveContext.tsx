// The server-push channel. Maintains one WebSocket to /ws with
// exponential-backoff reconnect, and exposes:
//   status     – connecting | online | offline (topbar badge)
//   metrics    – latest snapshot   (stat tiles)
//   history    – recent snapshots  (live chart)
//   activities – recent server events (activity feed)
// Connection transitions surface as toast bubbles, but only when a
// previously-live connection drops — a backend that is simply not
// running yet retries quietly.
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { Metrics } from "../lib/api";
import { useToast } from "./ToastContext";

export interface Activity {
  kind: string;
  message: string;
  timestampMs: number;
}

export type ConnectionStatus = "connecting" | "online" | "offline";

interface LiveContextValue {
  status: ConnectionStatus;
  metrics: Metrics | null;
  history: Metrics[];
  activities: Activity[];
}

type ServerEvent =
  | { type: "hello"; message: string; timestampMs: number }
  | { type: "metrics"; data: Metrics }
  | { type: "activity"; kind: string; message: string; timestampMs: number };

const HISTORY_LIMIT = 90; // ~3 minutes at one snapshot per 2s
const ACTIVITY_LIMIT = 50;

const LiveContext = createContext<LiveContextValue | null>(null);

export function LiveProvider({ children }: { children: ReactNode }) {
  const { push } = useToast();
  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [history, setHistory] = useState<Metrics[]>([]);
  const [activities, setActivities] = useState<Activity[]>([]);
  const wasOnline = useRef(false);
  const pushRef = useRef(push);
  pushRef.current = push;

  useEffect(() => {
    let socket: WebSocket | null = null;
    let reconnectTimer: number | undefined;
    let attempts = 0;
    let disposed = false;

    const prepend = (activity: Activity) =>
      setActivities((current) => [activity, ...current.slice(0, ACTIVITY_LIMIT - 1)]);

    const connect = () => {
      const proto = location.protocol === "https:" ? "wss" : "ws";
      socket = new WebSocket(`${proto}://${location.host}/ws`);
      setStatus("connecting");

      socket.onopen = () => {
        attempts = 0;
        setStatus("online");
        if (wasOnline.current) {
          pushRef.current("success", "Live connection restored");
        }
        wasOnline.current = true;
      };

      socket.onmessage = (message) => {
        let event: ServerEvent;
        try {
          event = JSON.parse(message.data as string) as ServerEvent;
        } catch {
          return;
        }
        if (event.type === "metrics") {
          setMetrics(event.data);
          setHistory((current) => [
            ...current.slice(-(HISTORY_LIMIT - 1)),
            event.data,
          ]);
        } else if (event.type === "activity") {
          prepend({
            kind: event.kind,
            message: event.message,
            timestampMs: event.timestampMs,
          });
        } else if (event.type === "hello") {
          prepend({
            kind: "socket",
            message: event.message,
            timestampMs: event.timestampMs,
          });
        }
      };

      socket.onclose = () => {
        if (disposed) return;
        setStatus("offline");
        if (wasOnline.current) {
          pushRef.current("warning", "Live connection lost — reconnecting…");
          wasOnline.current = false;
        }
        const delay = Math.min(15_000, 1000 * 2 ** attempts);
        attempts += 1;
        reconnectTimer = window.setTimeout(connect, delay);
      };
    };

    connect();

    return () => {
      disposed = true;
      if (reconnectTimer !== undefined) clearTimeout(reconnectTimer);
      socket?.close();
    };
  }, []);

  return (
    <LiveContext.Provider value={{ status, metrics, history, activities }}>
      {children}
    </LiveContext.Provider>
  );
}

export function useLive(): LiveContextValue {
  const ctx = useContext(LiveContext);
  if (!ctx) throw new Error("useLive must be used inside <LiveProvider>");
  return ctx;
}
