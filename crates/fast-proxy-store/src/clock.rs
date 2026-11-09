use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as unix milliseconds, the unit all timestamps use.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
