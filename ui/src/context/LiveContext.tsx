// The server-push channel. Maintains one WebSocket to /ws with
// exponential-backoff reconnect, and exposes:
//   status     – connecting | online | offline (topbar badge)
//   traffic    – latest traffic sample   (stat tiles)
//   history    – recent traffic samples  (live chart)
//   activities – recent server events (activity feed)
// A configChanged event (someone, maybe another admin, changed the
// configuration) refetches every list and detail that shows it. An
// endpointState event (an endpoint became unhealthy, was ejected, or came
// back) refetches the live endpoint status and the upstream health summary.
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
import { useQueryClient } from "@tanstack/react-query";
import type { Traffic } from "../lib/api";
import { refreshConfigData, refreshEndpointStatus } from "../lib/refreshConfigData";
import { useToast } from "./ToastContext";

export interface Activity {
  kind: string;
  message: string;
  timestampMs: number;
}

export type ConnectionStatus = "connecting" | "online" | "offline";

interface LiveContextValue {
  status: ConnectionStatus;
  traffic: Traffic | null;
  history: Traffic[];
  activities: Activity[];
}

type ServerEvent =
  | { type: "hello"; message: string; timestampMs: number }
  | { type: "traffic"; data: Traffic }
  | { type: "configChanged"; version: number; summary: string; timestampMs: number }
  | { type: "activity"; kind: string; message: string; timestampMs: number }
  | {
      type: "endpointState";
      subsystem: string;
      upstream: string;
      endpoint: string;
      change: "unhealthy" | "healthy" | "ejected" | "recovered";
      detail: string | null;
      timestampMs: number;
    };

const HISTORY_LIMIT = 90; // ~3 minutes at one sample per 2s
const ACTIVITY_LIMIT = 50;

const LiveContext = createContext<LiveContextValue | null>(null);

export function LiveProvider({ children }: { children: ReactNode }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const queryClientRef = useRef(queryClient);
  queryClientRef.current = queryClient;
  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  const [traffic, setTraffic] = useState<Traffic | null>(null);
  const [history, setHistory] = useState<Traffic[]>([]);
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
        if (event.type === "traffic") {
          setTraffic(event.data);
          setHistory((current) => [
            ...current.slice(-(HISTORY_LIMIT - 1)),
            event.data,
          ]);
        } else if (event.type === "configChanged") {
          void refreshConfigData(queryClientRef.current);
        } else if (event.type === "endpointState") {
          void refreshEndpointStatus(queryClientRef.current);
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
    <LiveContext.Provider value={{ status, traffic, history, activities }}>
      {children}
    </LiveContext.Provider>
  );
}

export function useLive(): LiveContextValue {
  const ctx = useContext(LiveContext);
  if (!ctx) throw new Error("useLive must be used inside <LiveProvider>");
  return ctx;
}
