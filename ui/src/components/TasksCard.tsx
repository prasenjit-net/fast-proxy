// The example REST service end to end, via TanStack Query: a useQuery
// list plus useMutation create/toggle/delete, each patching the ["tasks"]
// cache directly on success (no full refetch). Submitting an empty title
// intentionally reaches the server, which rejects it with a 400 —
// demonstrating the error-bubble pipeline.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useToast } from "../context/ToastContext";
import { api, type Task } from "../lib/api";
import { IconCheck, IconPlus, IconTrash } from "../icons";

export default function TasksCard() {
  const { push, notifyError } = useToast();
  const queryClient = useQueryClient();
  const [title, setTitle] = useState("");

  const tasksQuery = useQuery({ queryKey: ["tasks"], queryFn: api.listTasks });

  useEffect(() => {
    // Deliberately keyed on isError alone: a toast should fire once per
    // error transition, not once per render of the same error.
    if (tasksQuery.isError) notifyError(tasksQuery.error);
  }, [tasksQuery.isError, tasksQuery.error, notifyError]);

  const createMutation = useMutation({
    mutationFn: api.createTask,
    onSuccess: (task) => {
      queryClient.setQueryData<Task[]>(["tasks"], (current) => [task, ...(current ?? [])]);
      setTitle("");
    },
    onError: notifyError,
  });

  const toggleMutation = useMutation({
    mutationFn: api.toggleTask,
    onSuccess: (updated) => {
      queryClient.setQueryData<Task[]>(["tasks"], (current) =>
        (current ?? []).map((task) => (task.id === updated.id ? updated : task)),
      );
    },
    onError: notifyError,
  });

  const deleteMutation = useMutation({
    mutationFn: api.deleteTask,
    onSuccess: (_result, id) => {
      queryClient.setQueryData<Task[]>(["tasks"], (current) =>
        (current ?? []).filter((task) => task.id !== id),
      );
      push("info", "Task deleted");
    },
    onError: notifyError,
  });

  // useMutation happily queues concurrent calls, and the submit button's
  // `disabled` attribute doesn't stop a second Enter-key submit while the
  // first request is in flight — so this needs its own guard. A ref
  // (mutated synchronously, the instant mutate() is called) rather than
  // `createMutation.isPending`: that flag is updated through React's
  // regular render cycle, so two submits arriving back-to-back in the
  // same tick can both read it as `false` before either commits.
  const submitting = useRef(false);

  const add = (event: FormEvent) => {
    event.preventDefault();
    if (submitting.current) return;
    submitting.current = true;
    createMutation.mutate(title, {
      onSettled: () => {
        submitting.current = false;
      },
    });
  };

  const tasks = tasksQuery.data;

  return (
    <section className="card">
      <div className="card-head">
        <h2>Tasks</h2>
        <span className="card-hint">example REST service</span>
      </div>
      <form className="mb-3 flex gap-2" onSubmit={add}>
        <input
          className="input"
          placeholder="Add a task… (submit empty for a validation error)"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
        />
        <button className="btn btn-primary" disabled={createMutation.isPending} type="submit">
          <IconPlus size={16} /> Add
        </button>
      </form>
      {tasks === undefined ? (
        tasksQuery.isError ? (
          <p className="py-2 text-[0.86rem] text-err">Could not load tasks.</p>
        ) : (
          <div className="flex flex-col gap-3 py-2">
            <div className="skeleton" />
            <div className="skeleton" />
            <div className="skeleton" />
          </div>
        )
      ) : tasks.length === 0 ? (
        <p className="py-2 text-[0.86rem] text-ink-faint">No tasks yet — add one above.</p>
      ) : (
        <ul className="flex flex-col">
          {tasks.map((task) => (
            <li
              key={task.id}
              className="flex items-center gap-2.5 border-b border-line px-0.5 py-2 last:border-b-0"
            >
              <button
                className={`inline-flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-md border-[1.5px] transition-colors ${
                  task.done
                    ? "border-accent bg-accent text-on-accent"
                    : "border-line-strong text-transparent hover:border-accent"
                }`}
                onClick={() => toggleMutation.mutate(task.id)}
                aria-label={task.done ? "Reopen task" : "Complete task"}
              >
                {task.done ? <IconCheck size={13} /> : null}
              </button>
              <span
                className={`min-w-0 flex-1 text-[0.9rem] break-words ${
                  task.done ? "text-ink-faint line-through" : ""
                }`}
              >
                {task.title}
              </span>
              <button
                className="icon-btn danger"
                onClick={() => deleteMutation.mutate(task.id)}
                aria-label="Delete task"
              >
                <IconTrash size={16} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
