// A dialog over a dimmed backdrop. Closes on Escape and on a click on the
// backdrop itself (not on the dialog's own content).
import { useEffect, useId, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { IconX } from "../icons";

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** A wider dialog for forms with many fields. */
  wide?: boolean;
}

export default function Modal({ title, onClose, children, wide = false }: ModalProps) {
  const titleId = useId();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-scrim p-4 pt-[8vh]"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className={`card w-full shadow-2xl ${wide ? "max-w-[640px]" : "max-w-[460px]"}`}
      >
        <div className="mb-4 flex items-center gap-3">
          <h2 id={titleId} className="text-[1rem] font-semibold">
            {title}
          </h2>
          <button className="icon-btn ml-auto" onClick={onClose} aria-label="Close dialog">
            <IconX size={18} />
          </button>
        </div>
        {children}
      </div>
    </div>,
    document.body,
  );
}
