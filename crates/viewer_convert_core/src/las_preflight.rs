//! LAS/LAZ header preflight: bounds header-declared sizes by the real file length before the
//! `las`/`laz` decoders size buffers from them.

use std::io::{Read, Seek, SeekFrom};

/// Rejection reasons from [`preflight_las`]; each names the offending header field.
#[derive(Debug, thiserror::Error)]
pub enum LasPreflightError {
    #[error("LAS/LAZ input is truncated: {0}")]
    Truncated(&'static str),
    #[error("{0} extends past the end of the file")]
    VlrOutOfBounds(&'static str),
    #[error("EVLR record_length/start_of_first_evlr extends past the end of the file")]
    EvlrOutOfBounds,
    #[error(
        "LAZ chunk of {points} points x {point_size} B exceeds the {limit} B decompressed-chunk cap"
    )]
    ChunkTooLarge {
        points: u64,
        point_size: u64,
        limit: u64,
    },
    #[error(
        "header point count x point_data_record_length ({points} x {record_len} B) overflows u64"
    )]
    PointDataOverflow { points: u64, record_len: u64 },
    #[error(
        "header point count x point_data_record_length ({points} x {record_len} B) exceeds the {available} B after offset_to_point_data"
    )]
    PointDataExceedsFile {
        points: u64,
        record_len: u64,
        available: u64,
    },
    #[error("LAZ chunk table offset {0} lies outside the file")]
    ChunkTableOutOfBounds(u64),
    #[error("LAZ chunk table declares {chunks} chunks, more than the {limit} the file can hold")]
    ChunkCountExceedsPoints { chunks: u64, limit: u64 },
    #[error(
        "LAZ point_data_record_length ({record_len} B) differs from the laszip item sizes ({items_size} B)"
    )]
    LazRecordLengthMismatch { record_len: u64, items_size: u64 },
    #[error(
        "LAZ layered chunk declares a {size} B layer (layers total {total} B), but only {available} B remain in the chunk"
    )]
    LazLayerExceedsChunk {
        size: u64,
        total: u64,
        available: u64,
    },
    #[error("unsafe LAZ chunk table: {field} = {value} is outside the bound {bound}")]
    LazChunkTable {
        field: &'static str,
        value: u64,
        bound: u64,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Cap on decompressed bytes per LAZ chunk: `laz` allocates this much from table
/// or VLR values. Real files use 50 000 points x <=67 B (~3.4 MB); COPC <=1 M x 67 B (~67 MB).
pub const MAX_LAZ_CHUNK_BYTES: u64 = 256 << 20;
const LAS_MIN_HEADER: u64 = 227;
const VLR_HEADER_LEN: u64 = 54;
const EVLR_HEADER_LEN: u64 = 60;
const LASZIP_RECORD_ID: u16 = 22204;
const LASZIP_VARIABLE_CHUNK: u32 = u32::MAX;

/// Rejects chunks whose decompressed size exceeds [`MAX_LAZ_CHUNK_BYTES`].
pub fn check_chunk_bytes(points: u64, point_size: u64) -> Result<(), LasPreflightError> {
    match points.checked_mul(point_size) {
        Some(bytes) if bytes <= MAX_LAZ_CHUNK_BYTES => Ok(()),
        _ => Err(LasPreflightError::ChunkTooLarge {
            points,
            point_size,
            limit: MAX_LAZ_CHUNK_BYTES,
        }),
    }
}

fn min_point_record_len(format_id: u8) -> u64 {
    match format_id {
        1 => 28,
        2 => 26,
        3 => 34,
        4 => 57,
        5 => 63,
        6 => 30,
        7 => 36,
        8 => 38,
        9 => 59,
        10 => 67,
        _ => 20,
    }
}

fn read_array<const N: usize, R: Read + Seek>(
    reader: &mut R,
    at: u64,
) -> Result<[u8; N], LasPreflightError> {
    reader.seek(SeekFrom::Start(at))?;
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            LasPreflightError::Truncated("header or record shorter than declared")
        } else {
            error.into()
        }
    })?;
    Ok(buf)
}

fn read_u16_at<R: Read + Seek>(reader: &mut R, at: u64) -> Result<u16, LasPreflightError> {
    Ok(u16::from_le_bytes(read_array(reader, at)?))
}

fn read_u32_at<R: Read + Seek>(reader: &mut R, at: u64) -> Result<u32, LasPreflightError> {
    Ok(u32::from_le_bytes(read_array(reader, at)?))
}

fn read_u64_at<R: Read + Seek>(reader: &mut R, at: u64) -> Result<u64, LasPreflightError> {
    Ok(u64::from_le_bytes(read_array(reader, at)?))
}

/// Where the LAZ chunk table lives and what the header says about its contents.
/// Raw header facts only; decoding the table needs the `laz` crate (see
/// [`check_laz_chunk_table`], behind the `laz` feature), which bounds the decoded entries with these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LazChunkTableLayout {
    pub point_data_offset: u64,
    pub table_offset: u64,
    /// `None` for variable-size chunks.
    pub fixed_chunk_size: Option<u32>,
    /// Decompressed bytes per point as `laz` sizes buffers: the larger of the header
    /// record length and the laszip item sizes.
    pub point_size: u64,
    pub header_points: u64,
    /// Present when the chunks use the layered (point14) encoding.
    pub layered: Option<LayeredChunks>,
}

/// Raw layout of a layered LAZ chunk, derived from the laszip items:
/// the first point uncompressed (`first_point` B), a `u32` point count, one `u32` size per
/// layer (`layer_words`, in item order: point14 9, rgb14 1, rgbnir14 2, wavepacket14 1,
/// byte14 one per extra byte), then the layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayeredChunks {
    pub first_point: u64,
    pub layer_words: u64,
}

/// `laz` allocates each layer size read from the chunk before reading it, so every
/// size (and their sum) must fit in the chunk `start..end`. Returns the chunk's point
/// count and its length as the sequential reader consumes it.
pub fn check_layered_chunk<R: Read + Seek>(
    reader: &mut R,
    layered: LayeredChunks,
    start: u64,
    end: u64,
) -> Result<(u32, u64), LasPreflightError> {
    let header_len = layered
        .first_point
        .saturating_add(4)
        .saturating_add(layered.layer_words * 4);
    let body = start.saturating_add(header_len);
    if body > end {
        return Err(LasPreflightError::Truncated("LAZ layered chunk header"));
    }
    let count = read_u32_at(reader, start + layered.first_point)?;
    reader.seek(SeekFrom::Start(start + layered.first_point + 4))?;
    let mut sizes = vec![0u8; (layered.layer_words * 4) as usize];
    reader
        .read_exact(&mut sizes)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => {
                LasPreflightError::Truncated("LAZ layered chunk header")
            }
            _ => error.into(),
        })?;
    let available = end - body;
    let sizes: Vec<u64> = sizes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u64::from(u32::from_le_bytes(*word)))
        .collect();
    let total: u64 = sizes.iter().sum();
    if let Some(size) = sizes.iter().copied().find(|size| *size > available) {
        return Err(LasPreflightError::LazLayerExceedsChunk {
            size,
            total,
            available,
        });
    }
    if total > available {
        return Err(LasPreflightError::LazLayerExceedsChunk {
            size: total,
            total,
            available,
        });
    }
    Ok((count, header_len + total))
}

/// Walks chunks the way the sequential reader locates them (each starts where the
/// previous one's layers end) until the header's points are covered or `end` is reached.
fn walk_layered_chunks<R: Read + Seek>(
    reader: &mut R,
    layered: LayeredChunks,
    fixed_chunk_size: Option<u32>,
    start: u64,
    end: u64,
    points: u64,
) -> Result<(), LasPreflightError> {
    let (mut pos, mut remaining) = (start, points);
    while remaining > 0 && pos < end {
        let (count, len) = check_layered_chunk(reader, layered, pos, end)?;
        pos += len;
        remaining = remaining.saturating_sub(fixed_chunk_size.map_or(u64::from(count), u64::from));
    }
    if remaining > 0 {
        // The sequential reader would read the table or EVLR bytes as another chunk's layers.
        return Err(LasPreflightError::Truncated(
            "LAZ layered chunks hold fewer points than the header declares",
        ));
    }
    Ok(())
}

/// Bounds the decoded chunk-table entries by the header (`layout` comes from
/// [`preflight_las_layout`]) before `laz` sizes buffers from them, and checks that the table
/// covers the points the header declares: the sequential decompressor indexes the table by
/// chunk and traps past its end, which wasm32 cannot catch. Leaves the reader positioned
/// arbitrarily.
pub fn check_laz_chunk_table<R: Read + Seek>(
    reader: &mut R,
    layout: LazChunkTableLayout,
) -> Result<(), LasPreflightError> {
    use laz::laszip::ChunkTable;

    let reject = |field, value, bound| LasPreflightError::LazChunkTable {
        field,
        value,
        bound,
    };
    let offset_size = ChunkTable::OFFSET_SIZE as u64;
    let variable = layout.fixed_chunk_size.is_none();
    reader.seek(SeekFrom::Start(layout.table_offset))?;
    let table = ChunkTable::read(reader, variable)?;
    let data_bytes = layout
        .table_offset
        .saturating_sub(layout.point_data_offset + offset_size);
    let (mut sum_bytes, mut sum_points) = (0u64, 0u64);
    let mut chunk_start = layout.point_data_offset + offset_size;
    for entry in table.as_ref() {
        let bytes_bound = data_bytes.min(i32::MAX as u64);
        if entry.byte_count > bytes_bound {
            return Err(reject("chunk byte_count", entry.byte_count, bytes_bound));
        }
        sum_bytes += entry.byte_count;
        if sum_bytes > data_bytes {
            return Err(reject("chunk byte_count sum", sum_bytes, data_bytes));
        }
        // The parallel reader starts each chunk at the table's cumulative offset.
        let chunk_end = chunk_start + entry.byte_count;
        if let (Some(layered), true) = (layout.layered, entry.byte_count > 0) {
            check_layered_chunk(reader, layered, chunk_start, chunk_end)?;
        }
        chunk_start = chunk_end;
        if variable {
            if entry.point_count > layout.header_points {
                return Err(reject(
                    "chunk point_count",
                    entry.point_count,
                    layout.header_points,
                ));
            }
            // The header count is attacker-controlled, so it cannot bound allocations alone.
            check_chunk_bytes(entry.point_count, layout.point_size)?;
            sum_points += entry.point_count;
            if sum_points > layout.header_points {
                return Err(reject(
                    "chunk point_count sum",
                    sum_points,
                    layout.header_points,
                ));
            }
        }
    }
    // `laz` asserts that the table decodes at least as many points as the reader requests.
    let covered = match layout.fixed_chunk_size {
        None => sum_points,
        Some(chunk_size) => (table.as_ref().len() as u64).saturating_mul(u64::from(chunk_size)),
    };
    if covered < layout.header_points {
        return Err(reject(
            "chunk point_count total",
            covered,
            layout.header_points,
        ));
    }
    Ok(())
}

/// Bounds header-declared sizes by the real file length before the `las`/`laz`
/// crates allocate from them. Parses raw bytes; the reader is rewound to 0.
/// No `BufReader`: every access seeks, so a buffer would be discarded each time.
pub fn preflight_las<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<(), LasPreflightError> {
    preflight_las_layout(reader, file_len).map(|_| ())
}

/// Same checks as [`preflight_las`]; also returns the chunk table location for
/// LAZ inputs that carry one.
pub fn preflight_las_layout<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<Option<LazChunkTableLayout>, LasPreflightError> {
    let result = preflight_inner(reader, file_len);
    reader.seek(SeekFrom::Start(0))?;
    result
}

fn preflight_inner<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<Option<LazChunkTableLayout>, LasPreflightError> {
    if file_len < LAS_MIN_HEADER {
        return Err(LasPreflightError::Truncated(
            "file shorter than a LAS header",
        ));
    }
    if &read_array::<4, _>(reader, 0)? != b"LASF" {
        // Not LAS: the parser rejects it without header-driven allocations.
        return Ok(None);
    }
    let [_, minor] = read_array::<2, _>(reader, 24)?;
    let declared_header_size = u64::from(read_u16_at(reader, 94)?);
    if declared_header_size < LAS_MIN_HEADER || declared_header_size > file_len {
        return Err(LasPreflightError::Truncated("header_size"));
    }
    // `las` always reads the fixed header of the version; VLRs start after the larger one.
    let header_size = declared_header_size.max(match minor {
        0..=2 => 227,
        3 => 235,
        _ => 375,
    });
    if header_size > file_len {
        return Err(LasPreflightError::Truncated("version header"));
    }
    let point_data_offset = u64::from(read_u32_at(reader, 96)?);
    let vlr_count = read_u32_at(reader, 100)?;
    let format_byte = read_array::<1, _>(reader, 104)?[0];
    let format_id = format_byte & 0x3f;
    let record_len = u64::from(read_u16_at(reader, 105)?);
    let mut points = u64::from(read_u32_at(reader, 107)?);
    if minor >= 4 && points == 0 {
        // Same precedence as `las`: legacy count when non-zero.
        points = read_u64_at(reader, 247)?;
    }

    // `las` multiplies these unchecked (panics in debug builds).
    let point_bytes = points
        .checked_mul(record_len)
        .ok_or(LasPreflightError::PointDataOverflow { points, record_len })?;
    // `las` also adds the point data offset (LAS and LAZ alike), panicking in debug builds.
    point_data_offset
        .checked_add(point_bytes)
        .ok_or(LasPreflightError::PointDataOverflow { points, record_len })?;

    let mut laz_chunk_size = None;
    let mut laz_items_size = 0u64;
    let mut laz_declared_size = 0u64;
    let mut laz_layer_words: Option<u64> = None;
    let mut pos = header_size;
    for _ in 0..vlr_count {
        let end = pos + VLR_HEADER_LEN;
        if end > file_len {
            return Err(LasPreflightError::VlrOutOfBounds("VLR header"));
        }
        let user_id = read_array::<16, _>(reader, pos + 2)?;
        let record_id = read_u16_at(reader, pos + 18)?;
        let vlr_len = u64::from(read_u16_at(reader, pos + 20)?);
        if end + vlr_len > file_len {
            return Err(LasPreflightError::VlrOutOfBounds("VLR record_length"));
        }
        if laz_chunk_size.is_none()
            && record_id == LASZIP_RECORD_ID
            && user_id.starts_with(b"laszip encoded\0")
        {
            // laszip record: compressor u16, coder u16, version 4 B, options u32 (chunk_size at +12).
            if vlr_len < 16 {
                return Err(LasPreflightError::VlrOutOfBounds("laszip record"));
            }
            laz_chunk_size = Some(match read_u32_at(reader, end + 12)? {
                0 => LASZIP_VARIABLE_CHUNK,
                size => size,
            });
            // Items follow the 32-byte fixed part: count u16, then (type, size, version) u16 each.
            // A shorter record is rejected by `laz` itself.
            if vlr_len >= 34 {
                let count = u64::from(read_u16_at(reader, end + 32)?).min((vlr_len - 34) / 6);
                let mut words = 0u64;
                for item in 0..count {
                    let item_type = read_u16_at(reader, end + 34 + item * 6)?;
                    let declared = u64::from(read_u16_at(reader, end + 34 + item * 6 + 2)?);
                    laz_declared_size += declared;
                    // `laz` sizes some buffers by the declared size and the item decoders by
                    // their type, so both sums must agree with the record length.
                    let size = match item_type {
                        6 => 20,
                        7 | 12 => 8,
                        8 | 11 => 6,
                        9 | 13 => 29,
                        10 => 30,
                        _ => declared,
                    };
                    laz_items_size += size;
                    words += match item_type {
                        10 => 9,
                        11 | 13 => 1,
                        12 => 2,
                        14 => size,
                        _ => 0,
                    };
                }
                // `laz` picks the layered decoder from the first item's version.
                if count > 0 && matches!(read_u16_at(reader, end + 34 + 4)?, 3 | 4) {
                    laz_layer_words = Some(words);
                }
            }
        }
        pos = end + vlr_len;
    }

    if minor >= 4 {
        let first_evlr = read_u64_at(reader, 235)?;
        let evlr_count = read_u32_at(reader, 243)?;
        let mut pos = first_evlr;
        // `las` ignores EVLRs unless both fields are non-zero.
        let evlr_count = if first_evlr == 0 { 0 } else { evlr_count };
        for _ in 0..evlr_count {
            let end = pos
                .checked_add(EVLR_HEADER_LEN)
                .filter(|end| *end <= file_len)
                .ok_or(LasPreflightError::EvlrOutOfBounds)?;
            let len = read_u64_at(reader, pos + 20)?;
            pos = end
                .checked_add(len)
                .filter(|next| *next <= file_len)
                .ok_or(LasPreflightError::EvlrOutOfBounds)?;
        }
    }

    // `las` decides by the compression bit of the point format, not by the laszip VLR.
    if format_byte & 0x80 == 0 {
        laz_chunk_size = None;
    } else if laz_chunk_size.is_none() {
        // `las` fails with "laszip vlr not found" without allocating.
        return Ok(None);
    }
    if let Some(chunk_size) = laz_chunk_size {
        // `las` sizes buffers by the header record length, `laz` by the items; valid files agree.
        for items_size in [laz_declared_size, laz_items_size] {
            if items_size != record_len {
                return Err(LasPreflightError::LazRecordLengthMismatch {
                    record_len,
                    items_size,
                });
            }
        }
        let point_size = record_len.max(laz_items_size);
        let layered = laz_layer_words.map(|layer_words| LayeredChunks {
            first_point: laz_items_size,
            layer_words,
        });
        let fixed = (chunk_size != LASZIP_VARIABLE_CHUNK).then_some(chunk_size);
        if chunk_size != LASZIP_VARIABLE_CHUNK {
            check_chunk_bytes(u64::from(chunk_size), point_size)?;
        }
        let mut table = i64::from_le_bytes(read_array(reader, point_data_offset)?);
        if table <= point_data_offset as i64 {
            // Writer could not patch the offset; laz reads it from the file tail.
            table = i64::from_le_bytes(read_array(reader, file_len - 8)?);
            if table <= point_data_offset as i64 {
                if let Some(layered) = layered {
                    // No table: `laz` still reads the chunks sequentially up to the file end.
                    let first = point_data_offset + 8;
                    walk_layered_chunks(reader, layered, fixed, first, file_len, points)?;
                }
                return Ok(None);
            }
        }
        let table = table as u64;
        if table.checked_add(8).is_none_or(|end| end > file_len) {
            return Err(LasPreflightError::ChunkTableOutOfBounds(table));
        }
        let chunks = u64::from(read_u32_at(reader, table + 4)?);
        let by_points = if chunk_size == LASZIP_VARIABLE_CHUNK {
            points.min(file_len) + 1
        } else {
            points.div_ceil(u64::from(chunk_size)) + 1
        };
        // Every chunk starts with at least one uncompressed point record.
        let min_record = record_len.max(min_point_record_len(format_id)).max(1);
        let by_bytes = table
            .checked_sub(point_data_offset + 8)
            .map_or(0, |bytes| bytes / min_record);
        // A valid empty file may still carry one empty chunk.
        let by_bytes = if points == 0 {
            by_bytes.max(1)
        } else {
            by_bytes
        };
        let limit = by_points.min(by_bytes);
        if chunks > limit {
            return Err(LasPreflightError::ChunkCountExceedsPoints { chunks, limit });
        }
        if let Some(layered) = layered {
            let first = point_data_offset + 8;
            walk_layered_chunks(reader, layered, fixed, first, table, points)?;
        }
        return Ok(Some(LazChunkTableLayout {
            point_data_offset,
            table_offset: table,
            fixed_chunk_size: fixed,
            point_size,
            header_points: points,
            layered,
        }));
    }
    // Uncompressed: records are stored verbatim. EVLRs are not subtracted, so a file
    // `las` can read is never rejected.
    let available = file_len.saturating_sub(point_data_offset);
    if point_bytes > available {
        return Err(LasPreflightError::PointDataExceedsFile {
            points,
            record_len,
            available,
        });
    }
    Ok(None)
}

#[cfg(test)]
#[path = "las_preflight_tests.rs"]
mod tests;
