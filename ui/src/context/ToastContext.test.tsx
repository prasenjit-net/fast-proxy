import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../lib/api";
import { ToastProvider, useToast } from "./ToastContext";

function Consumer() {
  const { push, notifyError } = useToast();
  return (
    <div>
      <button onClick={() => push("success", "Saved cleanly")}>push-success</button>
      <button onClick={() => push("error", "Something broke")}>push-error</button>
      <button onClick={() => notifyError(new ApiError("NOT_FOUND", 404, "task 1 does not exist"))}>
        notify-api-error
      </button>
      <button onClick={() => notifyError(new Error("plain failure"))}>notify-plain-error</button>
    </div>
  );
}

function renderToasts() {
  return render(
    <ToastProvider>
      <Consumer />
    </ToastProvider>,
  );
}

describe("ToastContext", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders a toast with title and message on push", async () => {
    const user = userEvent.setup({ delay: null });
    renderToasts();
    await user.click(screen.getByText("push-success"));

    const region = screen.getByRole("status");
    expect(within(region).getByText("Success")).toBeInTheDocument();
    expect(within(region).getByText("Saved cleanly")).toBeInTheDocument();
  });

  it("formats an ApiError as 'CODE · status' with its message", async () => {
    const user = userEvent.setup({ delay: null });
    renderToasts();
    await user.click(screen.getByText("notify-api-error"));

    const region = screen.getByRole("status");
    expect(within(region).getByText("NOT_FOUND · 404")).toBeInTheDocument();
    expect(within(region).getByText("task 1 does not exist")).toBeInTheDocument();
  });

  it("falls back to a generic Error's message", async () => {
    const user = userEvent.setup({ delay: null });
    renderToasts();
    await user.click(screen.getByText("notify-plain-error"));

    expect(screen.getByText("plain failure")).toBeInTheDocument();
  });

  it("dismisses a toast when its close button is clicked", async () => {
    const user = userEvent.setup({ delay: null });
    renderToasts();
    await user.click(screen.getByText("push-success"));
    expect(screen.getByText("Saved cleanly")).toBeInTheDocument();

    await user.click(screen.getByLabelText("Dismiss notification"));
    expect(screen.queryByText("Saved cleanly")).not.toBeInTheDocument();
  });

  it("auto-dismisses success toasts after ~4.5s and error toasts after ~7s", async () => {
    const user = userEvent.setup({ delay: null });
    renderToasts();
    await user.click(screen.getByText("push-success"));
    await user.click(screen.getByText("push-error"));

    expect(screen.getByText("Saved cleanly")).toBeInTheDocument();
    expect(screen.getByText("Something broke")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4600);
    });
    expect(screen.queryByText("Saved cleanly")).not.toBeInTheDocument();
    expect(screen.getByText("Something broke")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2500);
    });
    expect(screen.queryByText("Something broke")).not.toBeInTheDocument();
  });

  it("keeps at most 5 toasts on screen", async () => {
    const user = userEvent.setup({ delay: null });
    renderToasts();
    for (let i = 0; i < 6; i += 1) {
      await user.click(screen.getByText("push-success"));
    }
    const region = screen.getByRole("status");
    expect(within(region).getAllByText("Saved cleanly")).toHaveLength(5);
  });
});
