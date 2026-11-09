// "Are you sure?" for destructive actions. The message should say exactly
// what will be removed or changed.
import type { ReactNode } from "react";
import Modal from "./Modal";

interface ConfirmDialogProps {
  title: string;
  /** What will happen, in plain words. */
  children: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  /** The action is running: both buttons are disabled. */
  busy?: boolean;
}

export default function ConfirmDialog({
  title,
  children,
  confirmLabel,
  onConfirm,
  onCancel,
  busy = false,
}: ConfirmDialogProps) {
  return (
    <Modal title={title} onClose={onCancel}>
      <div className="text-[0.9rem] leading-relaxed text-ink-muted">{children}</div>
      <div className="mt-5 flex justify-end gap-2">
        <button className="btn btn-secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button className="btn btn-danger" onClick={onConfirm} disabled={busy}>
          {confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
