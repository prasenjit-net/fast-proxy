//! Reading durations written the way people write them (`"250ms"`, `"45s"`).

use std::time::Duration;

use serde::{Deserialize, Deserializer};

/// Deserializes a human-readable duration.
pub(super) fn deserialize_duration<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    let raw = String::deserialize(deserializer)?;
    humantime::parse_duration(&raw).map_err(serde::de::Error::custom)
}
