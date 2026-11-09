// Notification bubbles. `push` shows one; `notifyError` converts any
// thrown value (usually an ApiError from lib/api.ts) into an error
// bubble carrying the backend's error code and message.
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import {
  IconAlertTriangle,
  IconCheckCircle,
  IconInfo,
  IconX,
  IconXCircle,
} from "../icons";
import { ApiError } from "../lib/api";

export type ToastKind = "info" | "success" | "warning" | "error";

interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  message: string;
}

interface ToastContextValue {
  push: (kind: ToastKind, message: string, title?: string) => void;
  notifyError: (error: unknown) => void;
  dismiss: (id: number) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

const TITLES: Record<ToastKind, string> = {
  info: "Heads up",
  success: "Success",
  warning: "Warning",
  error: "Error",
};

const ICONS: Record<ToastKind, ReactElement> = {
  info: <IconInfo size={18} />,
  success: <IconCheckCircle size={18} />,
  warning: <IconAlertTriangle size={18} />,
  error: <IconXCircle size={18} />,
};

const TONES: Record<ToastKind, { bar: string; icon: string }> = {
  info: { bar: "border-l-info", icon: "text-info" },
  success: { bar: "border-l-ok", icon: "text-ok" },
  warning: { bar: "border-l-warn", icon: "text-warn" },
  error: { bar: "border-l-err", icon: "text-err" },
};

let nextId = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const timers = useRef(new Map<number, number>());

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
    const timer = timers.current.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      timers.current.delete(id);
    }
  }, []);

  const push = useCallback(
    (kind: ToastKind, message: string, title?: string) => {
      const id = nextId++;
      setToasts((current) => [
        ...current.slice(-4), // keep at most 5 on screen
        { id, kind, message, title: title ?? TITLES[kind] },
      ]);
      const timer = window.setTimeout(
        () => dismiss(id),
        kind === "error" ? 7000 : 4500,
      );
      timers.current.set(id, timer);
    },
    [dismiss],
  );

  const notifyError = useCallback(
    (error: unknown) => {
      if (error instanceof ApiError) {
        const status = error.status > 0 ? ` · ${error.status}` : "";
        push("error", error.message, `${error.code}${status}`);
      } else if (error instanceof Error) {
        push("error", error.message);
      } else {
        push("error", String(error));
      }
    },
    [push],
  );

  const value = useMemo(
    () => ({ push, notifyError, dismiss }),
    [push, notifyError, dismiss],
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div
        className="fixed top-[72px] right-4 z-100 flex w-[min(380px,calc(100vw-32px))] flex-col gap-2 max-sm:right-4 max-sm:left-4 max-sm:w-auto"
        role="status"
        aria-live="polite"
      >
        {toasts.map((toast) => (
          <div
            key={toast.id}
            className={`toast-in flex items-start gap-2.5 rounded-lg border border-line border-l-[3px] bg-surface p-3 shadow-lg ${TONES[toast.kind].bar}`}
          >
            <span className={`mt-0.5 inline-flex ${TONES[toast.kind].icon}`}>
              {ICONS[toast.kind]}
            </span>
            <div className="flex min-w-0 flex-col">
              <strong className="text-[0.82rem] font-semibold">{toast.title}</strong>
              <span className="text-[0.84rem] break-words text-ink-muted">
                {toast.message}
              </span>
            </div>
            <button
              className="ml-auto cursor-pointer rounded p-0.5 text-ink-faint hover:bg-surface-2 hover:text-ink"
              onClick={() => dismiss(toast.id)}
              aria-label="Dismiss notification"
            >
              <IconX size={14} />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used inside <ToastProvider>");
  return ctx;
}
