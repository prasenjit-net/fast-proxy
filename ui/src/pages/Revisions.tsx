import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import Badge from "../components/Badge";
import ConfigViewer from "../components/ConfigViewer";
import ConfirmDialog from "../components/ConfirmDialog";
import EmptyState from "../components/EmptyState";
import Modal from "../components/Modal";
import WriteOnly from "../components/WriteOnly";
import { useToast } from "../context/ToastContext";
import { IconRotateCcw } from "../icons";
import { api } from "../lib/api";
import { timeAgo } from "../lib/format";
import { queryClient } from "../lib/queryClient";
import { refreshConfigData } from "../lib/refreshConfigData";
import type { RevisionSummary } from "../lib/types";

function RevisionView({ revision, onClose }: { revision: RevisionSummary; onClose: () => void }) {
  const query = useQuery({
    queryKey: ["revision", revision.version],
    queryFn: () => api.revision(revision.version),
  });
  return (
    <Modal wide title={`Revision ${revision.version}`} onClose={onClose}>
      <p className="mb-4 text-[0.86rem] text-ink-muted">{revision.summary}</p>
      {query.isError ? (
        <p className="text-[0.9rem] text-err">Could not load this revision: {query.error.message}</p>
      ) : query.data ? (
        <ConfigViewer subsystems={query.data.data.config.subsystems} />
      ) : (
        <div className="flex items-center gap-3 text-ink-faint">
          <div className="spinner" /> Loading…
        </div>
      )}
    </Modal>
  );
}

export default function RevisionsPage() {
  const { push, notifyError } = useToast();
  const [viewing, setViewing] = useState<RevisionSummary | null>(null);
  const [rollingBack, setRollingBack] = useState<RevisionSummary | null>(null);

  const query = useInfiniteQuery({
    queryKey: ["revisions"],
    queryFn: ({ pageParam }) => api.revisions(pageParam),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
  });

  const rollback = useMutation({
    mutationFn: (revision: RevisionSummary) => api.rollback(revision.version),
    onSuccess: async (result, revision) => {
      await refreshConfigData(queryClient);
      push("success", `Rolled back to revision ${revision.version} (now revision ${result.version})`);
      setRollingBack(null);
    },
    onError: (error) => {
      setRollingBack(null);
      notifyError(error);
    },
  });

  const pages = query.data?.pages ?? [];
  const current = pages[0]?.version;
  const revisions = pages.flatMap((page) => page.data);

  let body;
  if (query.isError) {
    body = <p className="text-[0.9rem] text-err">Could not load revisions: {query.error.message}</p>;
  } else if (!query.data) {
    body = (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  } else if (revisions.length === 0) {
    body = (
      <EmptyState title="No changes yet">
        Every change you make to subsystems, hosts, upstreams and routes is recorded here, and any
        earlier state can be restored.
      </EmptyState>
    );
  } else {
    body = (
      <>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-[0.88rem]">
            <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
              <tr>
                <th className="py-2 pr-4 font-medium">Version</th>
                <th className="py-2 pr-4 font-medium">Change</th>
                <th className="py-2 pr-4 font-medium">When</th>
                <th className="py-2" />
              </tr>
            </thead>
            <tbody>
              {revisions.map((revision) => (
                <tr key={revision.version} className="border-t border-line align-middle">
                  <td className="py-2.5 pr-4 font-mono">
                    {revision.version}{" "}
                    {revision.version === current ? <Badge tone="accent">current</Badge> : null}
                  </td>
                  <td className="py-2.5 pr-4">{revision.summary}</td>
                  <td
                    className="py-2.5 pr-4 text-ink-faint"
                    title={new Date(revision.createdAtMs).toLocaleString()}
                  >
                    {timeAgo(revision.createdAtMs)}
                  </td>
                  <td className="py-2.5 text-right whitespace-nowrap">
                    <button className="btn btn-ghost btn-sm" onClick={() => setViewing(revision)}>
                      View
                    </button>
                    {revision.version !== current ? (
                      <WriteOnly>
                        <button
                          className="btn btn-secondary btn-sm ml-1"
                          onClick={() => setRollingBack(revision)}
                        >
                          <IconRotateCcw size={14} /> Roll back
                        </button>
                      </WriteOnly>
                    ) : null}
                  </td>
                </tr>
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
              Load older revisions
            </button>
          </div>
        ) : null}
      </>
    );
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Revisions</h2>
        <span className="card-hint">the last 100 changes are kept</span>
      </div>
      {body}
      {viewing ? <RevisionView revision={viewing} onClose={() => setViewing(null)} /> : null}
      {rollingBack ? (
        <ConfirmDialog
          title={`Roll back to revision ${rollingBack.version}?`}
          confirmLabel="Roll back"
          busy={rollback.isPending}
          onCancel={() => setRollingBack(null)}
          onConfirm={() => rollback.mutate(rollingBack)}
        >
          <p>
            The whole configuration will be replaced by how it was at revision{" "}
            <b>{rollingBack.version}</b> ({rollingBack.summary}). It is recorded as a new revision, so
            you can undo it by rolling back again.
          </p>
        </ConfirmDialog>
      ) : null}
    </section>
  );
}
