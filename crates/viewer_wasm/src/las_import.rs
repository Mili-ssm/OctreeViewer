use std::io::{Read, Seek};

use las::{PointData, PointDataBuilder, Reader};
use thiserror::Error;

const MAX_POINTS_PER_CHUNK: u64 = 65_536;
const MAX_INPUT_BYTES_PER_CHUNK: usize = 8 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum LasImportError {
    #[error(transparent)]
    Las(#[from] las::Error),
    #[error("the requested point batch size must be greater than zero")]
    EmptyBatch,
    #[error("LAS header has non-finite coordinate bounds")]
    InvalidBounds,
    #[error("point at local coordinates {0:?} exceeds the viewer's f32 coordinate range")]
    PositionOutOfRange([f64; 3]),
    #[error("the LAS/LAZ stream cannot continue after a failed batch")]
    StreamFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LasPointChunk {
    pub positions: Vec<f32>,
    pub colors: Vec<u8>,
    pub processed_points: u64,
    pub total_points: u64,
    pub origin: [f64; 3],
    pub done: bool,
}

/// Incremental LAS/LAZ reader. The source must support synchronous random access.
/// Browser callers should create it inside a dedicated worker using `FileReaderSync`.
pub struct LasPointStream {
    reader: Reader,
    buffer: PointData,
    origin: [f64; 3],
    total_points: u64,
    processed_points: u64,
    record_len: usize,
    done: bool,
    failed: bool,
}

impl LasPointStream {
    pub fn new<R>(source: R) -> Result<Self, LasImportError>
    where
        R: Read + Seek + Send + Sync + 'static,
    {
        let reader = Reader::new(source)?;
        let bounds = reader.header().bounds();
        let origin = [bounds.min.x, bounds.min.y, bounds.min.z];
        if origin.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err(LasImportError::InvalidBounds);
        }

        let total_points = reader.header().number_of_points();
        let record_len = usize::from(reader.header().point_format().len());
        let buffer = PointDataBuilder::new().for_header(reader.header()).build();

        Ok(Self {
            reader,
            buffer,
            origin,
            total_points,
            processed_points: 0,
            record_len,
            done: total_points == 0,
            failed: false,
        })
    }

    pub fn origin(&self) -> [f64; 3] {
        self.origin
    }

    pub fn total_points(&self) -> u64 {
        self.total_points
    }

    pub fn processed_points(&self) -> u64 {
        self.processed_points
    }

    pub fn read_next(&mut self, requested_points: u64) -> Result<LasPointChunk, LasImportError> {
        if requested_points == 0 {
            return Err(LasImportError::EmptyBatch);
        }
        if self.failed {
            return Err(LasImportError::StreamFailed);
        }

        let mut chunk = LasPointChunk {
            positions: Vec::new(),
            colors: Vec::new(),
            processed_points: self.processed_points,
            total_points: self.total_points,
            origin: self.origin,
            done: self.done,
        };
        if self.done {
            return Ok(chunk);
        }

        let max_by_bytes = (MAX_INPUT_BYTES_PER_CHUNK / self.record_len.max(1)).max(1) as u64;
        let point_limit = requested_points.min(MAX_POINTS_PER_CHUNK).min(max_by_bytes);
        self.reader.fill_points(point_limit, &mut self.buffer)?;

        if self.buffer.is_empty() {
            self.done = true;
            chunk.done = true;
            return Ok(chunk);
        }

        let count = self.buffer.len();
        chunk.positions.reserve(count.saturating_mul(3));
        chunk.colors.reserve(count.saturating_mul(3));

        let conversion = (|| {
            let coordinates = self.buffer.x().zip(self.buffer.y()).zip(self.buffer.z());
            if let Some(rgb) = self.buffer.rgb() {
                for (((x, y), z), (red, green, blue)) in coordinates.zip(rgb) {
                    push_position(&mut chunk.positions, [x, y, z], self.origin)?;
                    chunk.colors.extend_from_slice(&[
                        (red >> 8) as u8,
                        (green >> 8) as u8,
                        (blue >> 8) as u8,
                    ]);
                }
            } else {
                for (((x, y), z), intensity) in coordinates.zip(self.buffer.intensity()) {
                    push_position(&mut chunk.positions, [x, y, z], self.origin)?;
                    let gray = (intensity >> 8) as u8;
                    chunk.colors.extend_from_slice(&[gray, gray, gray]);
                }
            }
            Ok::<(), LasImportError>(())
        })();
        if let Err(error) = conversion {
            self.failed = true;
            return Err(error);
        }

        self.processed_points += count as u64;
        self.done = (count as u64) < point_limit || self.processed_points >= self.total_points;
        chunk.processed_points = self.processed_points;
        chunk.done = self.done;
        Ok(chunk)
    }
}

fn push_position(
    positions: &mut Vec<f32>,
    world_position: [f64; 3],
    origin: [f64; 3],
) -> Result<(), LasImportError> {
    let local = std::array::from_fn(|axis| world_position[axis] - origin[axis]);
    let local_f32 = local.map(|coordinate| coordinate as f32);
    if local.iter().any(|coordinate| !coordinate.is_finite())
        || local_f32.iter().any(|coordinate| !coordinate.is_finite())
    {
        return Err(LasImportError::PositionOutOfRange(local));
    }
    positions.extend_from_slice(&local_f32);
    Ok(())
}
