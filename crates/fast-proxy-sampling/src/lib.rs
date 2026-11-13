//! Tail-based request sampling (ADRs 0020 to 0023): the decision after the
//! response, the rolling latency window, the per-client and storm guards, the
//! rate limiter, the redactor, the sample encoding and the batched hand-off to
//! a sink. No HTTP types and no database: the data plane feeds it plain facts
//! and the sample service implements [`SampleSink`].

mod batcher;
mod bucket;
mod budget;
mod client_errors;
mod decide;
mod facts;
mod latency_window;
mod ndjson;
mod policy;
mod query;
mod rate_limiter;
mod raw;
mod redact;
mod sample;
mod sampler;
mod sink;
mod storm_guard;

#[cfg(test)]
mod redact_tests;
#[cfg(test)]
mod sample_tests;
#[cfg(test)]
mod sampler_tests;

pub use batcher::Batcher;
pub use budget::{BodyBudget, BodyReservation};
pub use client_errors::{ClientErrors, ClientVerdict, SAMPLE_EVERY};
pub use decide::{decide, DecisionState};
pub use facts::{
    Answer, Attempt, AttemptLog, FailureKind, Outcome, RequestHead, UnusualFlags, MAX_ATTEMPTS,
};
pub use latency_window::{bucket_of, bucket_upper_ms, LatencyWindow, MIN_REQUESTS};
pub use ndjson::write_ndjson;
pub use policy::SamplingPolicy;
pub use query::{SampleQuery, MAX_LIMIT};
pub use rate_limiter::RateLimiter;
pub use raw::{AttemptDetail, CapturedBody, RawSample, Suppressed};
pub use redact::{
    RedactedBody, Redactor, BUILT_IN_BODY_FIELDS, BUILT_IN_HEADERS, BUILT_IN_QUERY_PARAMS, REDACTED,
};
pub use sample::{
    cap, Sample, SampleAttempt, SampleBody, SampleRequest, SampleResponse, SampleSuppressed,
    SampleUpstream, MAX_HEADERS, MAX_HEADER_VALUE_BYTES, MAX_PATH_BYTES,
};
pub use sampler::{
    Sampler, SamplerConfig, SamplerRuntime, SamplerStats, StatsReading, SubmitError,
};
pub use sink::{SampleSink, SinkError};
pub use storm_guard::{Admission, StormGuard, StormKey};
