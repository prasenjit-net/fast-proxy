import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { act } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToastProvider } from "../context/ToastContext";
import type { Task } from "../lib/api";
import TasksCard from "./TasksCard";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: {
      listTasks: vi.fn(),
      createTask: vi.fn(),
      toggleTask: vi.fn(),
      deleteTask: vi.fn(),
    },
  };
});

const { api } = await import("../lib/api");

function renderCard() {
  // A fresh QueryClient per render keeps the cache isolated between tests;
  // retry: false keeps failures deterministic instead of retried/delayed.
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <TasksCard />
      </ToastProvider>
    </QueryClientProvider>,
  );
}

const seedTask: Task = {
  id: 1,
  title: "Existing task",
  done: false,
  createdAt: new Date().toISOString(),
};

beforeEach(() => {
  // The mocked `api` module is shared across tests in this file, so its
  // vi.fn()s (unlike vi.spyOn spies) need an explicit reset each time —
  // vi.restoreAllMocks() in the global setup only resets spies.
  vi.clearAllMocks();
  vi.mocked(api.listTasks).mockResolvedValue([seedTask]);
});

describe("TasksCard", () => {
  it("lists tasks fetched on mount", async () => {
    renderCard();
    expect(await screen.findByText("Existing task")).toBeInTheDocument();
  });

  it("shows an empty-state message when there are no tasks", async () => {
    vi.mocked(api.listTasks).mockResolvedValue([]);
    renderCard();
    expect(await screen.findByText("No tasks yet — add one above.")).toBeInTheDocument();
  });

  // Regression test for the double-submit bug: the submit button's
  // `disabled` attribute doesn't stop a second Enter-key form submission
  // while the first request is still in flight. TasksCard guards this with
  // a ref (set synchronously the instant mutate() is called) rather than
  // createMutation.isPending, specifically so two submits fired back to
  // back in the same tick — as these are — can't both slip through before
  // either one's state update has been processed by React.
  it("only sends one request when the form is submitted twice before the first resolves", async () => {
    let resolveCreate!: (task: Task) => void;
    const pending = new Promise<Task>((resolve) => {
      resolveCreate = resolve;
    });
    vi.mocked(api.createTask).mockReturnValue(pending);

    renderCard();
    await screen.findByText("Existing task");

    const input = screen.getByPlaceholderText(/Add a task/);
    const form = input.closest("form")!;
    await userEvent.type(input, "New task");

    // Both fire in the same synchronous burst — the ref guard (not React
    // state) is what makes the second one a no-op regardless of exactly
    // when TanStack Query gets around to invoking the mutationFn.
    await act(async () => {
      fireEvent.submit(form);
      fireEvent.submit(form); // simulates a fast double Enter before the request resolves
    });

    expect(api.createTask).toHaveBeenCalledTimes(1);

    resolveCreate({
      id: 2,
      title: "New task",
      done: false,
      createdAt: new Date().toISOString(),
    });

    expect(await screen.findByText("New task")).toBeInTheDocument();
    // Only one insertion — not two — landed in the list.
    expect(screen.getAllByText("New task")).toHaveLength(1);
  });

  it("allows submitting again after the previous request completes", async () => {
    vi.mocked(api.createTask).mockResolvedValue({
      id: 3,
      title: "Second task",
      done: false,
      createdAt: new Date().toISOString(),
    });

    renderCard();
    await screen.findByText("Existing task");

    const input = screen.getByPlaceholderText(/Add a task/);
    const form = input.closest("form")!;
    await userEvent.type(input, "Second task");
    await act(async () => {
      fireEvent.submit(form);
    });

    expect(api.createTask).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Second task")).toBeInTheDocument();
  });
});
