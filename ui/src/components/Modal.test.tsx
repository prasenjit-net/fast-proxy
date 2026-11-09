import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import ConfirmDialog from "./ConfirmDialog";
import Modal from "./Modal";

describe("Modal", () => {
  it("shows a titled dialog with its content", () => {
    render(
      <Modal title="Edit route" onClose={vi.fn()}>
        <p>Form goes here</p>
      </Modal>,
    );
    const dialog = screen.getByRole("dialog", { name: "Edit route" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByText("Form goes here")).toBeInTheDocument();
  });

  it("closes on Escape, the close button, and a click on the backdrop", async () => {
    const onClose = vi.fn();
    render(
      <Modal title="Dialog" onClose={onClose}>
        <p>content</p>
      </Modal>,
    );
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);

    await userEvent.click(screen.getByRole("button", { name: "Close dialog" }));
    expect(onClose).toHaveBeenCalledTimes(2);

    await userEvent.click(screen.getByRole("dialog").parentElement as HTMLElement);
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it("does not close when the dialog's own content is clicked", async () => {
    const onClose = vi.fn();
    render(
      <Modal title="Dialog" onClose={onClose}>
        <p>content</p>
      </Modal>,
    );
    await userEvent.click(screen.getByText("content"));
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("ConfirmDialog", () => {
  it("states what will happen and calls the matching handler", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmDialog title="Delete shop?" confirmLabel="Delete" onConfirm={onConfirm} onCancel={onCancel}>
        This removes <b>2 routes</b>.
      </ConfirmDialog>,
    );
    expect(screen.getByText("2 routes")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables both buttons while the action runs", () => {
    render(
      <ConfirmDialog title="Delete?" confirmLabel="Delete" onConfirm={vi.fn()} onCancel={vi.fn()} busy>
        x
      </ConfirmDialog>,
    );
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
  });
});
