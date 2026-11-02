//! `viewer_convert verify`: the global checks that opening a `v7.5-pre` file no longer makes
//! (R5-Q3). Opening validates the metadata block and the root page; verifying reads every page.

use std::{fs::File, path::Path};

use viewer_format::{FormatReader, FormatReaderLimits, FormatVersion, PagedStats, probe_format};

use crate::error::ConvertError;

/// Verifying is an offline pass that holds every page, so it accepts far more nodes and blocks
/// than the interactive reader's defaults (2 M each).
const VERIFY_MAX_COUNT: u64 = 1 << 28;

fn verify_limits() -> Result<FormatReaderLimits, ConvertError> {
    let defaults = FormatReaderLimits::default();
    Ok(FormatReaderLimits::new(
        256 * 1024 * 1024,
        256 * 1024 * 1024,
        VERIFY_MAX_COUNT,
        VERIFY_MAX_COUNT,
        256,
        65_535,
        1_048_576,
    )?
    .with_max_attribute_ranges(VERIFY_MAX_COUNT * 16)?
    .with_page_limits(defaults.max_page_bytes(), defaults.max_resident_pages())?)
}

/// What a successful [`verify_file`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub version: FormatVersion,
    /// Page statistics; `None` for revisions whose whole index is checked at open.
    pub paged: Option<PagedStats>,
    pub block_count: u64,
}

/// Loads every page of `path` (a `.oct`) and runs the global checks: each page matches the node
/// that points to it, child pages tile the page section, payloads leave no gaps, every block has
/// one owner and the declared totals hold. Older revisions check their whole index at open, so
/// verifying them is opening them. Payload bytes are not read.
pub fn verify_file(path: &Path) -> Result<VerifyReport, ConvertError> {
    let mut file = File::open(path)?;
    let version = probe_format(&mut file)?.version;
    if version.uses_pages() {
        let stats = viewer_format::verify_paged(&mut file, &verify_limits()?)?;
        return Ok(VerifyReport {
            version,
            block_count: stats.blocks,
            paged: Some(stats),
        });
    }
    let reader = FormatReader::with_limits(file, verify_limits()?)?;
    Ok(VerifyReport {
        version,
        block_count: reader.block_storage_stats()?.len() as u64,
        paged: None,
    })
}
