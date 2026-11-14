//! Samples as newline-delimited JSON, for the export.

use std::io::{self, Write};

use crate::sample::Sample;

/// Writes each sample as one JSON line.
pub fn write_ndjson<'a, W: Write>(
    out: &mut W,
    samples: impl IntoIterator<Item = &'a Sample>,
) -> io::Result<()> {
    for sample in samples {
        out.write_all(sample.to_detail_json().as_bytes())?;
        out.write_all(b"\n")?;
    }
    Ok(())
}
