// Who is using the admin UI. The provider asks the server who the token (if
// any) belongs to and shows the sign-in page until the answer is a person: an
// admin plane with authentication off answers without a token, and the page
// then goes straight in. A `viewer` token gets a read-only UI: write controls
// are hidden, not just refused (see `WriteOnly`). When the server stops
// accepting the token, the user is sent back to sign in.
import { useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import Logo from "../components/Logo";
import SignInPage from "../pages/SignIn";
import { api, ApiError, type WhoAmI } from "../lib/api";
import { clearToken, setToken, setUnauthorizedHandler } from "../lib/session";

export interface SessionValue {
  who: WhoAmI;
  /** May change configuration (an `admin` token, or authentication is off). */
  canWrite: boolean;
  /** May manage tokens and export the audit log. */
  isAdmin: boolean;
  signOut: () => void;
}

export const SessionContext = createContext<SessionValue | null>(null);

type State =
  | { kind: "checking" }
  | { kind: "unreachable"; message: string }
  | { kind: "signed-out"; notice?: string }
  | { kind: "signed-in"; who: WhoAmI };

const SESSION_ENDED =
  "Your session ended: the token was revoked, expired, or is no longer accepted. Sign in again.";

export function SessionProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<State>({ kind: "checking" });

  const probe = useCallback(async () => {
    setState({ kind: "checking" });
    try {
      const { data } = await api.whoami();
      setState({ kind: "signed-in", who: data });
    } catch (thrown) {
      if (thrown instanceof ApiError && thrown.status === 401) {
        setState({ kind: "signed-out" });
      } else {
        setState({
          kind: "unreachable",
          message: thrown instanceof Error ? thrown.message : String(thrown),
        });
      }
    }
  }, []);

  useEffect(() => {
    setUnauthorizedHandler(() => {
      queryClient.clear();
      setState({ kind: "signed-out", notice: SESSION_ENDED });
    });
    void probe();
    return () => setUnauthorizedHandler(null);
  }, [probe, queryClient]);

  const signIn = useCallback(async (token: string) => {
    const { data } = await api.whoami(token);
    setToken(token);
    setState({ kind: "signed-in", who: data });
  }, []);

  const signOut = useCallback(() => {
    clearToken();
    queryClient.clear();
    setState({ kind: "signed-out" });
  }, [queryClient]);

  const value = useMemo<SessionValue | null>(() => {
    if (state.kind !== "signed-in") return null;
    return {
      who: state.who,
      canWrite: state.who.role === "admin",
      isAdmin: state.who.role === "admin",
      signOut,
    };
  }, [state, signOut]);

  if (state.kind === "checking") {
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-4 p-8">
        <Logo size={52} />
        <div className="spinner" aria-label="Loading" />
      </div>
    );
  }
  if (state.kind === "unreachable") {
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-4 p-8 text-center">
        <Logo size={52} />
        <h1 className="text-lg font-semibold">Cannot reach the server</h1>
        <p className="max-w-md text-ink-muted">{state.message}</p>
        <button className="btn btn-primary" onClick={() => void probe()}>
          Retry
        </button>
      </div>
    );
  }
  if (state.kind === "signed-out") {
    return <SignInPage notice={state.notice} onSignIn={signIn} />;
  }
  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

export function useSession(): SessionValue {
  const ctx = useContext(SessionContext);
  if (!ctx) throw new Error("useSession must be used inside <SessionProvider>");
  return ctx;
}

/** Whether write controls should be shown. */
export function useCanWrite(): boolean {
  return useSession().canWrite;
}
