//! `viewer_convert verify`: the global checks that opening a V8 file no longer makes (paged index
//! since `v7.5-pre`, R5-Q3). Opening validates the metadata block and the root page; verifying
//! reads every page.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use viewer_convert_core::error::ConvertError;
use viewer_format::{FormatError, FormatReaderLimits, FormatVersion, PagedStats, probe_format};

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
/// one owner and the declared totals hold. If the `.octattrs` companion exists next to `path`,
/// its footer must name the same index. Payload bytes are not read.
pub fn verify_file(path: &Path) -> Result<VerifyReport, ConvertError> {
    let mut file = File::open(path)?;
    // A retired family fails somewhere in the checks below; the envelope then names it.
    verify_open(path, &mut file).map_err(|error| match error {
        ConvertError::Format(error) => {
            ConvertError::Format(viewer_format::explain_open_failure(&mut file, error))
        }
        other => other,
    })
}

fn verify_open(path: &Path, file: &mut File) -> Result<VerifyReport, ConvertError> {
    let version = probe_format(file)?.version;
    let stats = viewer_format::verify_paged(file, &verify_limits()?)?;
    let companion = crate::publish::output_attributes_path(path);
    if companion.exists() {
        check_companion_footer(file, &mut File::open(companion)?)?;
    }
    Ok(VerifyReport {
        version,
        block_count: stats.blocks,
        paged: Some(stats),
    })
}

const FOOTER_BYTES: u64 = 40;

fn footer<R: Read + Seek>(
    file: &mut R,
) -> Result<([u8; FOOTER_BYTES as usize], u64), ConvertError> {
    let length = file.seek(SeekFrom::End(0))?;
    let start = length
        .checked_sub(FOOTER_BYTES)
        .ok_or(ConvertError::Format(FormatError::FileTooShort))?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = [0; FOOTER_BYTES as usize];
    file.read_exact(&mut bytes)?;
    Ok((bytes, length))
}

/// Both footers carry the index offset, length and CRC32 (bytes 8..24 and 32..36); the companion's
/// declared file length (bytes 24..32) must be its real length.
fn check_companion_footer<R: Read + Seek>(
    oct: &mut R,
    attributes: &mut R,
) -> Result<(), ConvertError> {
    let (primary, _) = footer(oct)?;
    let (companion, length) = footer(attributes)?;
    if primary[..8] != companion[..8]
        || primary[8..24] != companion[8..24]
        || primary[32..36] != companion[32..36]
        || u64::from_le_bytes(companion[24..32].try_into().expect("8 bytes")) != length
    {
        return Err(ConvertError::Format(FormatError::InvalidMetadata(
            "attribute companion does not match primary index",
        )));
    }
    Ok(())
}
