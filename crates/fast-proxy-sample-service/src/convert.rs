//! A redacted [`Sample`] as the store's row: the short columns the database
//! filters on and the JSON detail.

use fast_proxy_sampling::Sample;
use fast_proxy_store::repository::samples::NewSample;

/// The row to store for `sample`.
pub(crate) fn to_row(sample: &Sample) -> NewSample {
    NewSample {
        at_ms: sample.at_ms,
        instance: sample.instance.clone(),
        request_id: sample.request_id.clone(),
        triggers: sample.trigger_set(),
        subsystem: sample.subsystem.clone(),
        route: sample.route.clone(),
        client: sample.client.clone(),
        method: sample.request.method.clone(),
        status: sample.response.status,
        total_ms: sample.total_ms,
        detail: sample.to_detail_json(),
    }
}
