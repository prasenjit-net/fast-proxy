// The sampled requests: the unusual ones the proxy kept, newest first. An
// admin's page (a sample holds client addresses and, if a route asks, bodies);
// a viewer is told what it is and sees the settings and counts.
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Fragment, useState, type FormEvent } from "react";
import Badge from "../components/Badge";
import ConfirmDialog from "../components/ConfirmDialog";
import EmptyState from "../components/EmptyState";
import FormField from "../components/FormField";
import SampleDetailView from "../components/sampling/SampleDetailView";
import { useSession } from "../context/SessionContext";
import { useToast } from "../context/ToastContext";
import { api } from "../lib/api";
import { formatBytes, formatDurationMs, formatNumber, timeAgo } from "../lib/format";
import { downloadSamplesExport } from "../lib/samplesExport";
import { TRIGGERS } from "../lib/samplingValidation";
import type { SampleFilters, SampleTrigger } from "../lib/types";
import { useSampling } from "../lib/useSampling";

const SINCE_CHOICES: { value: string; label: string; ms: number }[] = [
  { value: "any", label: "Any time", ms: 0 },
  { value: "hour", label: "Last hour", ms: 3_600_000 },
  { value: "day", label: "Last 24 hours", ms: 86_400_000 },
  { value: "week", label: "Last 7 days", ms: 7 * 86_400_000 },
];

interface Applied {
  trigger: "" | SampleTrigger;
  statusClass: string;
  subsystem: string;
  route: string;
  client: string;
  minMs: string;
  since: string;
}

const NO_FILTERS: Applied = {
  trigger: "",
  statusClass: "",
  subsystem: "",
  route: "",
  client: "",
  minMs: "",
  since: "any",
};

const LIVE_REFRESH_MS = 5000;

function filtersOf(applied: Applied, now: number): SampleFilters {
  const reach = SINCE_CHOICES.find((choice) => choice.value === applied.since)?.ms ?? 0;
  const minMs = Number(applied.minMs);
  return {
    trigger: applied.trigger || undefined,
    statusClass: applied.statusClass ? Number(applied.statusClass) : undefined,
    subsystem: applied.subsystem.trim() || undefined,
    route: applied.route.trim() || undefined,
    client: applied.client.trim() || undefined,
    minMs: applied.minMs.trim() !== "" && Number.isFinite(minMs) ? minMs : undefined,
    sinceMs: reach ? now - reach : undefined,
  };
}

const label = (trigger: SampleTrigger) => TRIGGERS.find((entry) => entry.value === trigger)?.label ?? trigger;

function statusTone(status: number) {
  if (status >= 500) return "err";
  if (status >= 400) return "warn";
  return "neutral";
}

/** What a viewer sees: the page is an admin's, the settings and counts are not. */
function ViewerNotice() {
  const sampling = useSampling();
  const status = sampling.data?.data.status;
  return (
    <section className="card">
      <div className="card-head">
        <h2>Sampled requests</h2>
      </div>
      <EmptyState title="Samples are for administrators">
        A sample holds client addresses and, where a route asks for it, request and response
        bodies. Ask an administrator for a token with the admin role to read them.
        {status
          ? ` So far ${formatNumber(status.captured)} requests were kept and ${formatNumber(status.dropped)} were dropped by the limits.`
          : ""}
      </EmptyState>
    </section>
  );
}

function SamplesList() {
  const queryClient = useQueryClient();
  const { push, notifyError } = useToast();
  const [draft, setDraft] = useState<Applied>(NO_FILTERS);
  const [applied, setApplied] = useState<Applied>(NO_FILTERS);
  const [open, setOpen] = useState<number | null>(null);
  const [live, setLive] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [confirmingClear, setConfirmingClear] = useState(false);

  const query = useInfiniteQuery({
    queryKey: ["samples", applied],
    queryFn: ({ pageParam }) => api.samples(filtersOf(applied, Date.now()), pageParam),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
    refetchInterval: live ? LIVE_REFRESH_MS : false,
  });

  const clear = useMutation({
    mutationFn: api.clearSamples,
    onSuccess: async ({ deleted }) => {
      setConfirmingClear(false);
      setOpen(null);
      await queryClient.invalidateQueries({ queryKey: ["samples"] });
      push("success", `Deleted ${formatNumber(deleted)} samples`);
    },
    onError: (error) => {
      setConfirmingClear(false);
      notifyError(error);
    },
  });

  function apply(event: FormEvent) {
    event.preventDefault();
    setOpen(null);
    setApplied(draft);
  }

  async function exportSamples() {
    setExporting(true);
    try {
      await downloadSamplesExport(filtersOf(applied, Date.now()));
    } catch (error) {
      notifyError(error);
    } finally {
      setExporting(false);
    }
  }

  const samples = query.data?.pages.flatMap((page) => page.data) ?? [];
  const filtered = JSON.stringify(applied) !== JSON.stringify(NO_FILTERS);
  const set = (changes: Partial<Applied>) => setDraft({ ...draft, ...changes });

  let body;
  if (query.isError) {
    body = <p className="text-[0.9rem] text-err">Could not load the samples: {query.error.message}</p>;
  } else if (!query.data) {
    body = (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  } else if (samples.length === 0) {
    body = filtered ? (
      <EmptyState title="Nothing matches">No sample fits these filters. Widen them to see more.</EmptyState>
    ) : (
      <EmptyState title="No unusual requests kept yet">
        Requests that fail to reach an upstream, run slow, or look odd will appear here, with a small
        baseline of ordinary ones. Check the triggers under Settings.
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
                <th className="py-2 pr-4 font-medium">Why</th>
                <th className="py-2 pr-4 font-medium">Request</th>
                <th className="py-2 pr-4 font-medium">Status</th>
                <th className="py-2 pr-4 font-medium">Time</th>
                <th className="py-2 pr-4 font-medium">Client</th>
                <th className="py-2 font-medium" />
              </tr>
            </thead>
            <tbody>
              {samples.map((sample) => (
                <Fragment key={sample.id}>
                  <tr className="border-b border-line">
                    <td className="py-2.5 pr-4 whitespace-nowrap text-ink-faint" title={new Date(sample.atMs).toLocaleString()}>
                      {timeAgo(sample.atMs)}
                    </td>
                    <td className="py-2.5 pr-4">
                      <Badge tone="accent">{label(sample.primary)}</Badge>
                      {sample.triggers.length > 1 ? (
                        <span className="ml-1 text-[0.74rem] text-ink-faint">+{sample.triggers.length - 1}</span>
                      ) : null}
                    </td>
                    <td className="py-2.5 pr-4 [overflow-wrap:anywhere]">
                      <span className="font-mono text-[0.8rem]">{sample.method}</span> {sample.subsystem}{" "}
                      <span className="font-mono text-[0.8rem]">{sample.route}</span>
                      <span className="ml-1 text-[0.74rem] text-ink-faint">{formatBytes(sample.sizeBytes)}</span>
                    </td>
                    <td className="py-2.5 pr-4">
                      <Badge tone={statusTone(sample.status)}>{sample.status || "none"}</Badge>
                    </td>
                    <td className="py-2.5 pr-4 font-mono text-[0.8rem]">{formatDurationMs(sample.totalMs)}</td>
                    <td className="py-2.5 pr-4 font-mono text-[0.8rem]">{sample.client}</td>
                    <td className="py-2.5 text-right">
                      <button
                        className="btn btn-ghost btn-sm"
                        aria-expanded={open === sample.id}
                        onClick={() => setOpen(open === sample.id ? null : sample.id)}
                      >
                        {open === sample.id ? "Hide" : "Details"}
                      </button>
                    </td>
                  </tr>
                  {open === sample.id ? (
                    <tr className="border-b border-line bg-surface-2">
                      <td colSpan={7} className="px-3 py-3">
                        <SampleDetailView id={sample.id} />
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
              Load older samples
            </button>
          </div>
        ) : null}
      </>
    );
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Sampled requests</h2>
        <span className="card-hint">the unusual ones, from every instance</span>
        <div className="ml-auto flex items-center gap-2">
          <label className="flex items-center gap-1.5 text-[0.82rem]">
            <input type="checkbox" checked={live} onChange={(event) => setLive(event.target.checked)} /> Live
          </label>
          <button className="btn btn-secondary btn-sm" disabled={exporting} onClick={() => void exportSamples()}>
            Export NDJSON
          </button>
          <button className="btn btn-danger btn-sm" onClick={() => setConfirmingClear(true)}>
            Delete all
          </button>
        </div>
      </div>
      <form onSubmit={apply} className="mb-4 flex flex-wrap items-end gap-3">
        <FormField label="Why" htmlFor="samples-trigger">
          <select
            id="samples-trigger"
            className="input"
            value={draft.trigger}
            onChange={(event) => set({ trigger: event.target.value as Applied["trigger"] })}
          >
            <option value="">Any</option>
            {TRIGGERS.map((trigger) => (
              <option key={trigger.value} value={trigger.value}>
                {trigger.label}
              </option>
            ))}
          </select>
        </FormField>
        <FormField label="Status" htmlFor="samples-status">
          <select
            id="samples-status"
            className="input"
            value={draft.statusClass}
            onChange={(event) => set({ statusClass: event.target.value })}
          >
            <option value="">Any</option>
            {[2, 3, 4, 5].map((digit) => (
              <option key={digit} value={digit}>{`${digit}xx`}</option>
            ))}
          </select>
        </FormField>
        <FormField label="Subsystem" htmlFor="samples-subsystem">
          <input id="samples-subsystem" className="input" value={draft.subsystem} onChange={(event) => set({ subsystem: event.target.value })} />
        </FormField>
        <FormField label="Route" htmlFor="samples-route">
          <input id="samples-route" className="input" placeholder="/api" value={draft.route} onChange={(event) => set({ route: event.target.value })} />
        </FormField>
        <FormField label="Client" htmlFor="samples-client">
          <input id="samples-client" className="input" placeholder="203.0.113.7" value={draft.client} onChange={(event) => set({ client: event.target.value })} />
        </FormField>
        <FormField label="Slower than (ms)" htmlFor="samples-min">
          <input id="samples-min" className="input" inputMode="numeric" value={draft.minMs} onChange={(event) => set({ minMs: event.target.value })} />
        </FormField>
        <FormField label="When" htmlFor="samples-since">
          <select id="samples-since" className="input" value={draft.since} onChange={(event) => set({ since: event.target.value })}>
            {SINCE_CHOICES.map((choice) => (
              <option key={choice.value} value={choice.value}>
                {choice.label}
              </option>
            ))}
          </select>
        </FormField>
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
      </form>
      {body}
      {confirmingClear ? (
        <ConfirmDialog
          title="Delete all samples?"
          confirmLabel="Delete all"
          busy={clear.isPending}
          onCancel={() => setConfirmingClear(false)}
          onConfirm={() => clear.mutate()}
        >
          Every stored sample, from every instance, is removed for good. The settings stay. The
          deletion is recorded in the audit log.
        </ConfirmDialog>
      ) : null}
    </section>
  );
}

export default function SamplesPage() {
  const { isAdmin } = useSession();
  return isAdmin ? <SamplesList /> : <ViewerNotice />;
}
