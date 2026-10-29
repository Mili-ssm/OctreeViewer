//! Source inspection and decoder-panic containment, free of paths and the filesystem.

use std::io::{Read, Seek, SeekFrom};

use las::Reader;
use viewer_core::las::{LasPreflightError, LazChunkTableLayout};
use viewer_core::{AttributeDescriptor, CoordinateFrame, SpatialBounds};

use crate::error::ConvertError;

pub struct SourceInfo {
    pub bounds: SpatialBounds,
    pub frame: CoordinateFrame,
    pub point_count: u64,
    pub record_length: usize,
    pub point_format: las::point::Format,
    pub transforms: las::Vector<las::Transform>,
    pub has_crs: bool,
}

/// Bounds header-declared allocations by `len` and decoded LAZ chunk table entries by the
/// header, then opens the reader over `input` (positioned anywhere; it is rewound).
pub fn open_las_reader_from<R: Read + Seek + Send + Sync + 'static>(
    mut input: R,
    len: u64,
) -> Result<Reader, ConvertError> {
    if let Some(layout) = viewer_core::las::preflight_las_layout(&mut input, len)? {
        check_laz_chunk_table(&mut input, layout)?;
        input.seek(SeekFrom::Start(0))?;
    }
    contain_decoder_panic(|| Reader::new(std::io::BufReader::new(input)))?.map_err(Into::into)
}

/// Runs a call into the `las`/`laz` decoders and turns a panic into an error. Corrupt
/// arithmetic-coded data trips asserts inside `laz` that no header or chunk-table check can
/// predict. Keep the closure limited to the decoder call so panics in our own code stay loud.
/// The reader is left in an unknown state after a panic: callers must propagate the error and
/// drop it.
pub fn contain_decoder_panic<T>(call: impl FnOnce() -> T) -> Result<T, ConvertError> {
    contain_decoder_panic_at(None, call)
}

/// [`contain_decoder_panic`] tagging the error with the global index of the failing point.
pub fn contain_decoder_panic_at<T>(
    point_index: Option<u64>,
    call: impl FnOnce() -> T,
) -> Result<T, ConvertError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).map_err(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_owned());
        ConvertError::LasDecoderPanicked {
            message,
            point_index,
        }
    })
}

/// `Reader::fill_points` with decoder panics contained.
pub fn fill_points(
    reader: &mut Reader,
    count: u64,
    data: &mut las::PointData,
) -> Result<u64, ConvertError> {
    Ok(contain_decoder_panic(|| reader.fill_points(count, data))??)
}

/// `Reader::seek` with decoder panics contained.
pub fn seek_points(reader: &mut Reader, index: u64) -> Result<(), ConvertError> {
    Ok(contain_decoder_panic(|| reader.seek(index))??)
}

/// `Reader::seek` reporting `point_index` if the decoder panics.
pub fn seek_points_at(
    reader: &mut Reader,
    index: u64,
    point_index: u64,
) -> Result<(), ConvertError> {
    Ok(contain_decoder_panic_at(Some(point_index), || {
        reader.seek(index)
    })??)
}

/// Shared decoded chunk-table check; keeps the dedicated error for rejected entries.
/// `ChunkTable::read` runs in `contain_decoder_panic` because wasm cannot unwind.
fn check_laz_chunk_table<R: Read + Seek>(
    file: &mut R,
    layout: LazChunkTableLayout,
) -> Result<(), ConvertError> {
    match contain_decoder_panic(|| viewer_core::las::check_laz_chunk_table(file, layout))? {
        Err(LasPreflightError::LazChunkTable {
            field,
            value,
            bound,
        }) => Err(ConvertError::LazChunkTable {
            field,
            value,
            bound,
        }),
        other => other.map_err(Into::into),
    }
}

pub fn inspect_source(reader: &Reader) -> Result<SourceInfo, ConvertError> {
    inspect_header(reader.header())
}

pub fn inspect_header(header: &las::Header) -> Result<SourceInfo, ConvertError> {
    let source_bounds = header.bounds();
    let min = [
        source_bounds.min.x,
        source_bounds.min.y,
        source_bounds.min.z,
    ];
    let max = [
        source_bounds.max.x,
        source_bounds.max.y,
        source_bounds.max.z,
    ];
    let bounds = SpatialBounds::new(min, max).map_err(|_| ConvertError::InvalidHeaderBounds)?;
    Ok(SourceInfo {
        bounds,
        frame: CoordinateFrame::new(min)?,
        point_count: header.number_of_points(),
        record_length: usize::from(header.point_format().len()).max(1),
        point_format: *header.point_format(),
        transforms: *header.transforms(),
        has_crs: header.has_crs_vlrs(),
    })
}

pub fn source_schema(format: &las::point::Format) -> Vec<AttributeDescriptor> {
    viewer_core::las::las_attribute_schema(format.has_gps_time, format.has_color, format.has_nir)
}

/// Pairs columns with `las_attribute_schema` by position; the name check makes
/// a reordered schema fail loudly in tests instead of mislabelling data.
pub fn next_descriptor(
    descriptors: &mut impl Iterator<Item = AttributeDescriptor>,
    expected: &str,
) -> AttributeDescriptor {
    let descriptor = descriptors
        .next()
        .expect("LAS attribute schema covers every column");
    debug_assert_eq!(descriptor.name(), expected);
    descriptor
}

pub fn omitted_dimensions(format: &las::point::Format, has_crs: bool) -> Vec<String> {
    let mut dimensions = vec![
        "synthetic/key-point/withheld/overlap flags".to_owned(),
        "scan direction and flight-line edge flags".to_owned(),
    ];
    if format.is_extended {
        dimensions.push("scanner channel".to_owned());
    }
    if format.has_waveform {
        dimensions.push("waveform data".to_owned());
    }
    if format.extra_bytes > 0 {
        dimensions.push("extra point bytes".to_owned());
    }
    if has_crs {
        dimensions.push("CRS/WKT metadata".to_owned());
    }
    dimensions
}
