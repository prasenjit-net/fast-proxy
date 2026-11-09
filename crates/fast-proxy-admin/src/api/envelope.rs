use serde::Serialize;

/// Every config API response: the resource and the config version it
/// reflects (for a write, the revision that write created).
#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub data: T,
    pub version: u64,
}

/// The response to a delete, which has no resource to return.
#[derive(Debug, Serialize)]
pub struct VersionOnly {
    pub version: u64,
}
