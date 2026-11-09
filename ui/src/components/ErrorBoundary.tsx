// Catches render errors below it and shows a recoverable fallback
// instead of a white screen. Used app-wide in App.tsx and again around
// the crash demo on the Components page (nested boundaries recover the
// smallest possible subtree).
import { Component, type ErrorInfo, type ReactNode } from "react";
import { IconAlertTriangle } from "../icons";

interface Props {
  children: ReactNode;
  title?: string;
  onRetry?: () => void;
}

interface State {
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("ErrorBoundary caught:", error, info.componentStack);
  }

  private reset = () => {
    this.setState({ error: null });
    this.props.onRetry?.();
  };

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className="my-2 flex flex-col items-start gap-2.5 rounded-xl border border-err bg-err-soft p-5">
          <span className="text-err">
            <IconAlertTriangle size={28} />
          </span>
          <h2 className="text-[0.95rem] font-semibold">
            {this.props.title ?? "Something went wrong"}
          </h2>
          <p className="text-sm text-ink-muted">{this.state.error.message}</p>
          <div className="flex flex-wrap items-center gap-2.5">
            <button className="btn btn-secondary" onClick={this.reset}>
              Try again
            </button>
            <button className="btn btn-primary" onClick={() => window.location.reload()}>
              Reload app
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
