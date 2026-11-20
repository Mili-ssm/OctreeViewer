//! Spool records: layout, encoding, decoding and the per-child spool writer.

use std::io::{self, Read, Write};

use viewer_core::ColorRgb;
use viewer_core::las::{LasExtraByte, extra::MAX_EXTRA_BYTES_PER_POINT};

use crate::ConvertError;
use crate::pipeline::spool_store::{SpoolId, SpoolStore};
use crate::source::SourceInfo;

pub(crate) const MAX_SPOOL_RECORD_BYTES: usize = 55 + 1 + MAX_EXTRA_BYTES_PER_POINT;
pub(super) const CHILD_SPOOL_BUFFER_BYTES: usize = 32 * 1024;

/// All temporary files for a source share these attributes and transforms.
/// Source coordinates stay on the exact LAS integer grid; generated LOD centers use f64.
#[derive(Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent layout flags of the spool record"
)]
pub struct SpoolLayout {
    transforms: las::Vector<las::Transform>,
    pub(super) source_coordinates: bool,
    pub(super) has_gps_time: bool,
    pub(super) has_color: bool,
    pub(super) has_nir: bool,
    /// `v7.5-pre`: the record carries the `las_flags` byte and `extra_len` raw Extra Bytes.
    pub(super) has_flags: bool,
    pub(super) extra_len: u8,
    pub(super) is_extended: bool,
    /// `v7.4-pre`: source positions are snapped to the 2^-15 grid so that node and cell
    /// membership is decided on exact integers.
    snap_to_quanta: bool,
}

impl SpoolLayout {
    pub const fn is_global(self) -> bool {
        self.snap_to_quanta
    }

    pub fn source(source: &SourceInfo) -> Self {
        Self {
            transforms: source.transforms,
            source_coordinates: true,
            has_gps_time: source.point_format.has_gps_time,
            has_color: source.point_format.has_color,
            has_nir: source.point_format.has_nir,
            has_flags: source.las_extra_bytes.is_some(),
            // Checked against `MAX_EXTRA_BYTES_PER_POINT` by `SourceInfo::schema`.
            extra_len: source.las_extra_bytes.as_deref().map_or(0, |extras| {
                extras.iter().map(LasExtraByte::byte_len).sum::<usize>() as u8
            }),
            is_extended: source.point_format.is_extended,
            snap_to_quanta: source.global.is_some(),
        }
    }

    #[must_use]
    pub fn lod(self) -> Self {
        Self {
            source_coordinates: false,
            ..self
        }
    }

    #[inline]
    pub fn record_bytes(self) -> usize {
        let coordinates = if self.source_coordinates { 12 } else { 24 };
        coordinates
            + 15
            + usize::from(self.has_gps_time) * 8
            + usize::from(self.has_color) * 6
            + usize::from(self.has_nir) * 2
            + usize::from(self.has_flags)
            + usize::from(self.extra_len)
    }

    /// Offset of the RGB16 triplet in a record (valid when `has_color`).
    #[inline]
    pub(super) fn rgb_offset(self) -> usize {
        (if self.source_coordinates { 12 } else { 24 }) + 12 + usize::from(self.has_gps_time) * 8
    }

    /// The LAS coordinate before any snapping (header bounds are checked against it).
    #[inline]
    pub(super) fn exact_position(self, raw: [i32; 3]) -> [f64; 3] {
        [
            self.transforms.x.direct(raw[0]),
            self.transforms.y.direct(raw[1]),
            self.transforms.z.direct(raw[2]),
        ]
    }

    #[inline]
    pub(super) fn position(self, raw: [i32; 3]) -> [f64; 3] {
        let exact = self.exact_position(raw);
        if self.snap_to_quanta {
            exact.map(|value| {
                viewer_format::quantize_coordinate(value)
                    .map_or(value, viewer_format::quanta_to_units)
            })
        } else {
            exact
        }
    }
}

#[derive(Clone, Copy)]
pub struct SpoolPoint {
    pub(super) raw_position: [i32; 3],
    pub position: [f64; 3],
    pub(super) intensity: u16,
    pub(super) classification: u8,
    pub(super) return_number: u8,
    pub(super) returns_per_pulse: u8,
    pub(super) scan_angle: f32,
    pub(super) user_data: u8,
    pub(super) point_source_id: u16,
    pub(super) gps_time: f64,
    pub(super) rgb: [u16; 3],
    pub(super) nir: u16,
    pub(super) las_flags: u8,
    /// Raw Extra Bytes of the record; only the first `SpoolLayout::extra_len` are meaningful.
    pub(super) extra: [u8; MAX_EXTRA_BYTES_PER_POINT],
    pub(super) display_color: ColorRgb,
}

pub struct ChildSpool {
    layout: SpoolLayout,
    pub id: SpoolId,
    pub point_count: u64,
    buffer: Vec<u8>,
    /// The store spool exists: an empty child never creates one.
    pub created: bool,
    pub payload_written_bytes: Option<u64>,
}

impl ChildSpool {
    pub fn new(id: SpoolId, layout: SpoolLayout, collect_diagnostics: bool) -> Self {
        Self {
            layout,
            id,
            point_count: 0,
            buffer: Vec::with_capacity(CHILD_SPOOL_BUFFER_BYTES),
            created: false,
            payload_written_bytes: collect_diagnostics.then_some(0),
        }
    }

    #[inline]
    pub fn write_point(
        &mut self,
        store: &impl SpoolStore,
        point: &SpoolPoint,
    ) -> Result<(), ConvertError> {
        if self.buffer.len() + self.layout.record_bytes() > CHILD_SPOOL_BUFFER_BYTES {
            self.flush(store)?;
        }
        write_point(&mut self.buffer, point, self.layout)?;
        self.point_count += 1;
        Ok(())
    }

    /// Appends an encoded record of the spool's layout unchanged.
    #[inline]
    pub fn write_record(
        &mut self,
        store: &impl SpoolStore,
        record: &[u8],
    ) -> Result<(), ConvertError> {
        if self.buffer.len() + record.len() > CHILD_SPOOL_BUFFER_BYTES {
            self.flush(store)?;
        }
        self.buffer.extend_from_slice(record);
        self.point_count += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn flush(&mut self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if !self.created {
            store.create(self.id, self.layout.record_bytes())?;
            self.created = true;
        }
        store.append(self.id, &self.buffer)?;
        if let Some(bytes_written) = self.payload_written_bytes.as_mut() {
            *bytes_written = bytes_written
                .checked_add(self.buffer.len() as u64)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        self.buffer.clear();
        Ok(())
    }

    pub fn finish(&mut self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        self.flush(store)?;
        self.close_created(store)
    }

    /// Closes the spool without flushing the buffer (all-equal root: the children are discarded).
    pub fn close_created(&self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        if self.created {
            store.close(self.id)?;
        }
        Ok(())
    }
}

/// Appends whatever `write_source_points` hands over; each call is already a ≤ 32 KiB chunk.
pub struct StoreWriter<'a, S> {
    pub store: &'a S,
    pub id: SpoolId,
}

impl<S: SpoolStore> Write for StoreWriter<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.store
            .append(self.id, bytes)
            .map_err(|error| match error {
                ConvertError::Io(error) => error,
                other => io::Error::other(other),
            })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[inline]
pub fn write_point(
    writer: &mut impl Write,
    point: &SpoolPoint,
    layout: SpoolLayout,
) -> io::Result<()> {
    let mut record = [0_u8; MAX_SPOOL_RECORD_BYTES];
    let mut offset = 0;
    macro_rules! append_bytes {
        ($bytes:expr) => {{
            let bytes = $bytes;
            let end = offset + bytes.len();
            record[offset..end].copy_from_slice(&bytes);
            offset = end;
        }};
    }
    if layout.source_coordinates {
        for coordinate in point.raw_position {
            append_bytes!(coordinate.to_le_bytes());
        }
    } else {
        for coordinate in point.position {
            append_bytes!(coordinate.to_le_bytes());
        }
    }
    append_bytes!(point.intensity.to_le_bytes());
    append_bytes!([
        point.classification,
        point.return_number,
        point.returns_per_pulse,
    ]);
    append_bytes!(point.scan_angle.to_le_bytes());
    append_bytes!([point.user_data]);
    append_bytes!(point.point_source_id.to_le_bytes());
    if layout.has_gps_time {
        append_bytes!(point.gps_time.to_le_bytes());
    }
    if layout.has_color {
        for channel in point.rgb {
            append_bytes!(channel.to_le_bytes());
        }
    }
    if layout.has_nir {
        append_bytes!(point.nir.to_le_bytes());
    }
    if layout.has_flags {
        append_bytes!([point.las_flags]);
    }
    append_bytes!(&point.extra[..usize::from(layout.extra_len)]);
    append_bytes!([
        point.display_color.r,
        point.display_color.g,
        point.display_color.b,
    ]);
    writer.write_all(&record[..offset])
}

#[inline]
pub fn read_records(
    reader: &mut impl Read,
    buffer: &mut [u8],
    layout: SpoolLayout,
) -> io::Result<usize> {
    let mut bytes_read = 0;
    while bytes_read < buffer.len() {
        match reader.read(&mut buffer[bytes_read..]) {
            Ok(0) => break,
            Ok(count) => bytes_read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let record_bytes = layout.record_bytes();
    if !bytes_read.is_multiple_of(record_bytes) {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(bytes_read / record_bytes)
}

#[cfg(test)]
fn read_point_if_present(
    reader: &mut impl Read,
    layout: SpoolLayout,
) -> io::Result<Option<SpoolPoint>> {
    let mut buffer = [0_u8; MAX_SPOOL_RECORD_BYTES];
    let bytes = &mut buffer[..layout.record_bytes()];
    let read = reader.read(&mut bytes[..1])?;
    if read == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut bytes[1..])?;
    Ok(Some(decode_point(bytes, layout)))
}

#[cfg(test)]
pub(super) fn read_point(
    reader: &mut impl Read,
    layout: SpoolLayout,
) -> Result<SpoolPoint, ConvertError> {
    read_point_if_present(reader, layout)?
        .ok_or_else(|| ConvertError::Io(io::Error::from(io::ErrorKind::UnexpectedEof)))
}

// Hot loop: `inline(always)` saves ~0.55% Ir over `inline` (M1_R7 record, diagnosis).
#[expect(
    clippy::inline_always,
    reason = "hot loop: measured gain over `inline` (see the comment above)"
)]
#[inline(always)]
pub fn decode_point(bytes: &[u8], layout: SpoolLayout) -> SpoolPoint {
    let mut offset = 0;
    let mut raw_position = [0; 3];
    let position = if layout.source_coordinates {
        for coordinate in &mut raw_position {
            *coordinate =
                i32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed width"));
            offset += 4;
        }
        layout.position(raw_position)
    } else {
        std::array::from_fn(|_| read_f64(bytes, &mut offset))
    };
    let intensity = read_u16(bytes, &mut offset);
    let classification = bytes[offset];
    let return_number = bytes[offset + 1];
    let returns_per_pulse = bytes[offset + 2];
    offset += 3;
    let scan_angle = f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed width"));
    offset += 4;
    let user_data = bytes[offset];
    offset += 1;
    let point_source_id = read_u16(bytes, &mut offset);
    let gps_time = if layout.has_gps_time {
        read_f64(bytes, &mut offset)
    } else {
        0.0
    };
    let rgb = if layout.has_color {
        std::array::from_fn(|_| read_u16(bytes, &mut offset))
    } else {
        [0; 3]
    };
    let nir = if layout.has_nir {
        read_u16(bytes, &mut offset)
    } else {
        0
    };
    let las_flags = if layout.has_flags {
        offset += 1;
        bytes[offset - 1]
    } else {
        0
    };
    let mut extra = [0; MAX_EXTRA_BYTES_PER_POINT];
    let extra_len = usize::from(layout.extra_len);
    extra[..extra_len].copy_from_slice(&bytes[offset..offset + extra_len]);
    offset += extra_len;
    let display_color = ColorRgb::new(bytes[offset], bytes[offset + 1], bytes[offset + 2]);
    SpoolPoint {
        raw_position,
        position,
        intensity,
        classification,
        return_number,
        returns_per_pulse,
        scan_angle,
        user_data,
        point_source_id,
        gps_time,
        rgb,
        nir,
        las_flags,
        extra,
        display_color,
    }
}

/// Position of a source-layout record: the first half of [`decode_point`] without the rest.
#[expect(
    clippy::inline_always,
    reason = "hot loop: the same measured gain as `decode_point`"
)]
#[inline(always)]
pub fn decode_position(bytes: &[u8], layout: SpoolLayout) -> [f64; 3] {
    debug_assert!(layout.source_coordinates);
    let raw = std::array::from_fn(|axis| {
        i32::from_le_bytes(
            bytes[4 * axis..4 * axis + 4]
                .try_into()
                .expect("fixed width"),
        )
    });
    layout.position(raw)
}

#[inline]
fn read_u16(bytes: &[u8], offset: &mut usize) -> u16 {
    let value = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
    *offset += 2;
    value
}

#[inline]
fn read_f64(bytes: &[u8], offset: &mut usize) -> f64 {
    let value = f64::from_le_bytes(bytes[*offset..*offset + 8].try_into().expect("fixed width"));
    *offset += 8;
    value
}
