import type { ReactNode } from "react";

interface EmptyStateProps {
  title: string;
  children: ReactNode;
  action?: ReactNode;
}

/** What to show when a list has nothing in it yet: explain the first step. */
export default function EmptyState({ title, children, action }: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center gap-2 px-4 py-10 text-center">
      <h3 className="text-[1rem] font-semibold">{title}</h3>
      <p className="max-w-md text-[0.88rem] leading-relaxed text-ink-muted">{children}</p>
      {action ? <div className="mt-2">{action}</div> : null}
    </div>
  );
}
