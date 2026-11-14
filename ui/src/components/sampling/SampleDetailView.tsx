// One sampled request in full: what came in, what went out, and each attempt
// at an upstream. Everything shown was redacted before it was stored.
import { useQuery } from "@tanstack/react-query";
import { api } from "../../lib/api";
import { formatBytes, formatDurationMs, timeAgo } from "../../lib/format";
import type { SampleBody, SampleDetail } from "../../lib/types";

function Headers({ headers }: { headers: [string, string][] }) {
  if (headers.length === 0) return <p className="text-ink-faint">none kept</p>;
  return (
    <dl className="m-0 grid gap-x-3 font-mono text-[0.76rem] sm:grid-cols-[auto_1fr]">
      {headers.map(([name, value], index) => (
        <div key={`${name}-${index}`} className="contents">
          <dt className="text-ink-faint">{name}</dt>
          <dd className="m-0 [overflow-wrap:anywhere]">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function Body({ body, omitted }: { body: SampleBody | null; omitted?: string | null }) {
  if (!body) {
    return <p className="text-ink-faint">{omitted ? `not kept: ${omitted}` : "not kept"}</p>;
  }
  return (
    <>
      <pre className="m-0 max-h-64 overflow-auto rounded-md bg-surface p-2 font-mono text-[0.76rem] whitespace-pre-wrap">
        {body.text}
      </pre>
      {body.truncated ? <p className="text-[0.74rem] text-ink-faint">cut at the size limit</p> : null}
    </>
  );
}

function View({ sample }: { sample: SampleDetail }) {
  const { request, response } = sample;
  return (
    <div className="grid gap-4 text-[0.82rem] lg:grid-cols-2">
      <div className="flex flex-col gap-2">
        <h4 className="font-semibold">
          Request: <span className="font-mono">{request.method} {request.path}{request.query ? `?${request.query}` : ""}</span>
        </h4>
        <p className="text-ink-faint">
          {formatBytes(request.bytes)} from {sample.client}
          {request.unusual?.length ? `; unusual: ${request.unusual.join(", ")}` : ""}
        </p>
        <Headers headers={request.headers} />
        <Body body={request.body} />
      </div>
      <div className="flex flex-col gap-2">
        <h4 className="font-semibold">
          Response: <span className="font-mono">{response.status}</span> in {formatDurationMs(sample.totalMs)}
        </h4>
        <p className="text-ink-faint">{formatBytes(response.bytes)}</p>
        <Headers headers={response.headers} />
        <Body body={response.body} omitted={response.bodyOmitted} />
      </div>
      <div className="flex flex-col gap-1 lg:col-span-2">
        <h4 className="font-semibold">Upstream attempts</h4>
        {sample.upstream.attempts.length === 0 ? (
          <p className="text-ink-faint">none: the proxy answered itself</p>
        ) : (
          <ol className="m-0 list-decimal pl-5 font-mono text-[0.78rem]">
            {sample.upstream.attempts.map((attempt, index) => (
              <li key={index}>
                {attempt.endpoint}: {attempt.failure ?? `answered ${attempt.status}`}
              </li>
            ))}
          </ol>
        )}
        {sample.suppressed ? (
          <p className="text-ink-faint">
            {sample.suppressed.count} similar requests were not kept since {timeAgo(sample.suppressed.sinceMs)}.
          </p>
        ) : null}
        <p className="text-ink-faint">
          Request id <span className="font-mono">{sample.requestId}</span>, instance{" "}
          <span className="font-mono">{sample.instance}</span>
        </p>
      </div>
    </div>
  );
}

/** Loads and shows one sample. */
export default function SampleDetailView({ id }: { id: number }) {
  const query = useQuery({ queryKey: ["samples", "detail", id], queryFn: () => api.sample(id) });
  if (query.isError) return <p className="text-err">Could not load the sample: {query.error.message}</p>;
  if (!query.data) {
    return (
      <div className="flex items-center gap-3 text-ink-faint">
        <div className="spinner" /> Loading…
      </div>
    );
  }
  return <View sample={query.data.data} />;
}
