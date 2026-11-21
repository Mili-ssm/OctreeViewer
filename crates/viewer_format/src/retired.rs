//! Recognition of the format families retired with V8 (A4 of the V8 freeze).
//!
//! The readers of those families are gone or going, so a file that carries one fails at open with
//! [`FormatError::RetiredFormat`] ("reconvert it") instead of a generic header or footer error.
//! Recognition uses only the raw envelope bytes, never `FormatVersion`: the 8-byte footer magic
//! names each family alone, and the header's major and revision numbers name it when the footer
//! is lost.

use std::io::{Read, Seek, SeekFrom};

use crate::FormatError;
use crate::layout::{FOOTER_SIZE, HEADER_MAGIC, HEADER_SIZE};

/// Footer magic and name of every retired family. V8 final (`OCTVEND8`) is not listed.
const FOOTERS: [(&[u8; 8], &str); 13] = [
    (b"OCTVEND5", "v5"),
    (b"OCTVE61P", "v6.1-pre"),
    (b"OCTVE62P", "v6.2-pre"),
    (b"OCTVE63P", "v6.3-pre"),
    (b"OCTVE64P", "v6.4-pre"),
    (b"OCTVE71P", "v7.1-pre"),
    (b"OCTVE72P", "v7.2-pre"),
    (b"OCTVE73P", "v7.3-pre"),
    (b"OCTVE74P", "v7.4-pre"),
    (b"OCTVE75P", "v7.5-pre"),
    (b"OCTVEND7", "v7"),
    (b"OCTVE81P", "v8.1-pre"),
    (b"OCTVE82P", "v8.2-pre"),
];

/// Header major and revision (0 = final) of every retired family.
const HEADERS: [(u16, u32, &str); 13] = [
    (5, 0, "v5"),
    (6, 1, "v6.1-pre"),
    (6, 2, "v6.2-pre"),
    (6, 3, "v6.3-pre"),
    (6, 4, "v6.4-pre"),
    (7, 1, "v7.1-pre"),
    (7, 2, "v7.2-pre"),
    (7, 3, "v7.3-pre"),
    (7, 4, "v7.4-pre"),
    (7, 5, "v7.5-pre"),
    (7, 0, "v7"),
    (8, 1, "v8.1-pre"),
    (8, 2, "v8.2-pre"),
];

/// The retired family whose footer magic opens `footer` (the last 40 bytes of a file), if any.
pub fn retired_format_name(footer: &[u8]) -> Option<&'static str> {
    let magic = footer.get(..8)?;
    FOOTERS
        .iter()
        .find(|(candidate, _)| magic == candidate.as_slice())
        .map(|(_, name)| *name)
}

/// The retired family a header (the first 24 bytes of a file) declares, if any. Checks the magic,
/// major and revision only; a damaged checksum does not hide the family.
pub fn retired_header_name(header: &[u8]) -> Option<&'static str> {
    if header.get(..8)? != HEADER_MAGIC.as_slice() || header.len() < 20 {
        return None;
    }
    let major = u16::from_le_bytes(header[8..10].try_into().ok()?);
    let revision = u32::from_le_bytes(header[16..20].try_into().ok()?);
    HEADERS
        .iter()
        .find(|(m, r, _)| (*m, *r) == (major, revision))
        .map(|(_, _, name)| *name)
}

/// Turns a failed open into [`FormatError::RetiredFormat`] when the envelope bytes in hand name a
/// retired family (`header` and `footer` are the first 24 and last 40 bytes of the file, whichever
/// the caller has); any other error, and every I/O error, is returned unchanged. The bytes are
/// those the failed open already read, so this costs no extra read.
pub fn explain_retired(
    error: FormatError,
    header: Option<&[u8]>,
    footer: Option<&[u8]>,
) -> FormatError {
    if matches!(error, FormatError::Io(_) | FormatError::RetiredFormat(_)) {
        return error;
    }
    footer
        .and_then(retired_format_name)
        .or_else(|| header.and_then(retired_header_name))
        .map_or(error, FormatError::RetiredFormat)
}

/// Like [`explain_retired`] for a reader the caller still owns: reads its header and footer (best effort)
/// to name a retired family behind `error`. For tools that open files through other entry points.
pub fn explain_open_failure<R: Read + Seek>(reader: &mut R, error: FormatError) -> FormatError {
    if matches!(error, FormatError::Io(_) | FormatError::RetiredFormat(_)) {
        return error;
    }
    let header = read_at(reader, SeekFrom::Start(0), HEADER_SIZE as usize);
    let footer = read_at(
        reader,
        SeekFrom::End(-(FOOTER_SIZE as i64)),
        FOOTER_SIZE as usize,
    );
    explain_retired(error, header.as_deref(), footer.as_deref())
}

pub(crate) fn read_at<R: Read + Seek>(
    reader: &mut R,
    from: SeekFrom,
    length: usize,
) -> Option<Vec<u8>> {
    let mut bytes = vec![0; length];
    reader.seek(from).ok()?;
    reader.read_exact(&mut bytes).ok()?;
    Some(bytes)
}
