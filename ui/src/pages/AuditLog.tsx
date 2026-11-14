import { useInfiniteQuery } from "@tanstack/react-query";
import { Fragment, useState, type FormEvent } from "react";
import Badge from "../components/Badge";
import EmptyState from "../components/EmptyState";
import FormField from "../components/FormField";
import { useSession } from "../context/SessionContext";
import { useToast } from "../context/ToastContext";
import { api } from "../lib/api";
import { downloadAuditExport } from "../lib/auditExport";
import { timeAgo } from "../lib/format";
import type { AuditEntry, AuditFilters, AuditOutcome } from "../lib/types";

const TONE = { ok: "ok", denied: "err", failed: "warn" } as const;

/** How far back the "since" menu reaches, in milliseconds; 0 is any time. */
const SINCE_CHOICES: { value: string; label: string; ms: number }[] = [
  { value: "any", label: "Any time", ms: 0 },
  { value: "hour", label: "Last hour", ms: 3_600_000 },
  { value: "day", label: "Last 24 hours", ms: 86_400_000 },
  { value: "week", label: "Last 7 days", ms: 7 * 86_400_000 },
  { value: "month", label: "Last 30 days", ms: 30 * 86_400_000 },
];

interface Applied {
  actor: string;
  action: string;
  outcome: "" | AuditOutcome;
  since: string;
}

const NO_FILTERS: Applied = { actor: "", action: "", outcome: "", since: "any" };

function filtersOf(applied: Applied, now: number): AuditFilters {
  const reach = SINCE_CHOICES.find((choice) => choice.value === applied.since)?.ms ?? 0;
  return {
    actor: applied.actor.trim() || undefined,
    action: applied.action.trim() || undefined,
    outcome: applied.outcome || undefined,
    sinceMs: reach ? now - reach : undefined,
  };
}

function EntryDetail({ entry }: { entry: AuditEntry }) {
  return (
    <dl className="grid gap-x-6 gap-y-1 text-[0.8rem] sm:grid-cols-[auto_1fr]">
      <dt className="text-ink-faint">Request</dt>
      <dd className="font-mono">{entry.requestId || "—"}</dd>
      <dt className="text-ink-faint">Instance</dt>
      <dd className="font-mono">{entry.instance}</dd>
      <dt className="text-ink-faint">Revision</dt>
      <dd>{entry.revision ?? "none (nothing was committed)"}</dd>
      <dt className="text-ink-faint">Detail</dt>
      <dd>
        <pre className="m-0 overflow-x-auto font-mono whitespace-pre-wrap">
          {typeof entry.detail === "string" ? entry.detail : JSON.stringify(entry.detail, null, 2)}
        </pre>
      </dd>
    </dl>
  );
}

export default function AuditLogPage() {
  const { isAdmin } = useSession();
  const { notifyError } = useToast();
  const [draft, setDraft] = useState<Applied>(NO_FILTERS);
  const [applied, setApplied] = useState<Applied>(NO_FILTERS);
  const [open, setOpen] = useState<number | null>(null);
  const [exporting, setExporting] = useState(false);

  const query = useInfiniteQuery({
    queryKey: ["audit", applied],
    queryFn: ({ pageParam }) => api.audit(filtersOf(applied, Date.now()), pageParam),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
  });

  function apply(event: FormEvent) {
    event.preventDefault();
    setOpen(null);
    setApplied(draft);
  }

  async function exportLog() {
    setExporting(true);
    try {
      await downloadAuditExport(filtersOf(applied, Date.now()).sinceMs);
    } catch (error) {
      notifyError(error);
    } finally {
      setExporting(false);
    }
  }

  const entries = query.data?.pages.flatMap((page) => page.data) ?? [];
  const filtered = JSON.stringify(applied) !== JSON.stringify(NO_FILTERS);

  let body;
  if (query.isError) {
    body = <p className="text-[0.9rem] text-err">Could not load the audit log: {query.error.message}</p>;
  } else if (!query.data) {
    body = (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  } else if (entries.length === 0) {
    body = filtered ? (
      <EmptyState title="Nothing matches">No entry fits these filters. Widen them to see more.</EmptyState>
    ) : (
      <EmptyState title="Nothing recorded yet">
        Every change, and every refused attempt, will appear here with who made it.
      </EmptyState>
    );
  } else {
    body = (
      <>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[0.86rem]">
            <thead>
              <tr className="border-b border-line text-[0.74rem] tracking-wide text-ink-faint uppercase">
                <th className="py-2 pr-4 font-medium">When</th>
                <th className="py-2 pr-4 font-medium">Outcome</th>
                <th className="py-2 pr-4 font-medium">Who</th>
                <th className="py-2 pr-4 font-medium">Action</th>
                <th className="py-2 pr-4 font-medium">Target</th>
                <th className="py-2 font-medium" />
              </tr>
            </thead>
            <tbody>
              {entries.map((entry) => (
                <Fragment key={entry.id}>
                  <tr className="border-b border-line">
                    <td className="py-2.5 pr-4 whitespace-nowrap text-ink-faint" title={new Date(entry.atMs).toLocaleString()}>
                      {timeAgo(entry.atMs)}
                    </td>
                    <td className="py-2.5 pr-4">
                      <Badge tone={TONE[entry.outcome]} dot>
                        {entry.outcome}
                      </Badge>
                    </td>
                    <td className="py-2.5 pr-4">
                      <span className="font-medium">{entry.actor}</span>
                      <span className="ml-1 text-[0.76rem] text-ink-faint">{entry.source}</span>
                    </td>
                    <td className="py-2.5 pr-4 font-mono text-[0.8rem]">{entry.action}</td>
                    <td className="py-2.5 pr-4 [overflow-wrap:anywhere]">{entry.target}</td>
                    <td className="py-2.5 text-right">
                      <button
                        className="btn btn-ghost btn-sm"
                        aria-expanded={open === entry.id}
                        onClick={() => setOpen(open === entry.id ? null : entry.id)}
                      >
                        {open === entry.id ? "Hide" : "Details"}
                      </button>
                    </td>
                  </tr>
                  {open === entry.id ? (
                    <tr key={`${entry.id}-detail`} className="border-b border-line bg-surface-2">
                      <td colSpan={6} className="px-3 py-3">
                        <EntryDetail entry={entry} />
                      </td>
                    </tr>
                  ) : null}
                </Fragment>
              ))}
            </tbody>
          </table>
        </div>
        {query.hasNextPage ? (
          <div className="mt-4 flex justify-center">
            <button
              className="btn btn-secondary btn-sm"
              onClick={() => void query.fetchNextPage()}
              disabled={query.isFetchingNextPage}
            >
              Load older entries
            </button>
          </div>
        ) : null}
      </>
    );
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Audit log</h2>
        <span className="card-hint">who changed what, and who was refused</span>
        {isAdmin ? (
          <button
            className="btn btn-secondary btn-sm ml-auto"
            disabled={exporting}
            onClick={() => void exportLog()}
          >
            Export NDJSON
          </button>
        ) : null}
      </div>
      <form onSubmit={apply} className="filter-bar mb-4">
        <FormField label="Who" htmlFor="audit-actor">
          <input
            id="audit-actor"
            className="input"
            placeholder="a token name"
            value={draft.actor}
            onChange={(event) => setDraft({ ...draft, actor: event.target.value })}
          />
        </FormField>
        <FormField label="Action" htmlFor="audit-action">
          <input
            id="audit-action"
            className="input"
            placeholder="maintenance.set"
            value={draft.action}
            onChange={(event) => setDraft({ ...draft, action: event.target.value })}
          />
        </FormField>
        <FormField label="Outcome" htmlFor="audit-outcome">
          <select
            id="audit-outcome"
            className="input"
            value={draft.outcome}
            onChange={(event) => setDraft({ ...draft, outcome: event.target.value as Applied["outcome"] })}
          >
            <option value="">Any</option>
            <option value="ok">ok</option>
            <option value="denied">denied</option>
            <option value="failed">failed</option>
          </select>
        </FormField>
        <FormField label="When" htmlFor="audit-since">
          <select
            id="audit-since"
            className="input"
            value={draft.since}
            onChange={(event) => setDraft({ ...draft, since: event.target.value })}
          >
            {SINCE_CHOICES.map((choice) => (
              <option key={choice.value} value={choice.value}>
                {choice.label}
              </option>
            ))}
          </select>
        </FormField>
        <div className="flex gap-2">
          <button type="submit" className="btn btn-primary">
            Apply
          </button>
          {filtered ? (
            <button
              type="button"
              className="btn btn-ghost"
              onClick={() => {
                setDraft(NO_FILTERS);
                setApplied(NO_FILTERS);
              }}
            >
              Clear
            </button>
          ) : null}
        </div>
      </form>
      {body}
    </section>
  );
}
