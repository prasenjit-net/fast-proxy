//! A sample as the data plane hands it over: complete but not yet redacted.
//! Nothing here is ever stored; the sampler task turns it into a
//! [`Sample`](crate::Sample) first.

use fast_proxy_model::TriggerSet;

use crate::budget::BodyReservation;
use crate::facts::{Outcome, RequestHead};

/// How many candidates of one storm key were dropped before this sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suppressed {
    pub count: u32,
    pub since_ms: u32,
}

/// A captured body prefix with its content type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedBody {
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// The body was longer than what was captured.
    pub truncated: bool,
}

/// One upstream attempt with its endpoint resolved to an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptDetail {
    pub endpoint: String,
    /// The connectivity failure, in its API spelling; `None` for an answer.
    pub failure: Option<String>,
    pub status: Option<u16>,
}

/// Everything about one kept request, before redaction.
#[derive(Debug)]
pub struct RawSample {
    pub at_ms: i64,
    pub request_id: String,
    pub triggers: TriggerSet,
    pub subsystem: String,
    pub route: String,
    pub head: RequestHead,
    pub outcome: Outcome,
    pub attempts: Vec<AttemptDetail>,
    pub response_headers: Vec<(String, String)>,
    pub request_body: Option<CapturedBody>,
    pub response_body: Option<CapturedBody>,
    /// Why there is no body: `off`, `budget`, `content_type`, `upgrade`.
    pub body_omitted: Option<&'static str>,
    pub suppressed: Option<Suppressed>,
    /// Holds the body memory budget until this sample has been handled.
    pub reservation: Option<BodyReservation>,
}
