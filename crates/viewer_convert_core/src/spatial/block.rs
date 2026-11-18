//! A planned block and how it is read back from its spool.

use viewer_core::las::LasExtraByte;
use viewer_core::{AttributeColumn, AttributeValues, BlockId};
use viewer_format::SourcePointBlock;

use super::spool::{SpoolLayout, decode_point, read_records};
use crate::ConvertError;
use crate::pipeline::spool_store::{SPOOL_IO_BUFFER_BYTES, SpoolExtent, SpoolId, SpoolStore};
use crate::source::next_descriptor;

pub struct PlannedBlock {
    pub id: BlockId,
    pub(super) spool: SpoolId,
    pub(super) offset_points: u64,
    pub(super) point_count: usize,
    pub(super) layout: SpoolLayout,
    /// Descriptors of the Extra Byte columns (`v7.5-pre`); empty otherwise.
    pub(super) extras: std::sync::Arc<[LasExtraByte]>,
    pub is_source_points: bool,
    // Blocks sharing a spool stay in construction order when streamed. Only the
    // last range may release it; blocks are written in plan order, so by then every other
    // read of the spool has finished.
    pub(super) final_block_in_spool: bool,
}

impl PlannedBlock {
    pub fn point_count(&self) -> usize {
        self.point_count
    }

    pub fn payload_bytes(&self) -> Result<u64, ConvertError> {
        u64::try_from(self.point_count)
            .ok()
            .and_then(|points| points.checked_mul(self.layout.record_bytes() as u64))
            .ok_or(ConvertError::SizeOverflow)
    }

    pub fn release_spool_after_write(&self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        if self.final_block_in_spool {
            store.delete(self.spool)?;
        }
        Ok(())
    }

    pub fn read<S: SpoolStore>(
        &self,
        store: &S,
        frame: viewer_core::CoordinateFrame,
    ) -> Result<SourcePointBlock, ConvertError> {
        let record_bytes_wide = self.layout.record_bytes() as u64;
        let extent = SpoolExtent {
            offset: self
                .offset_points
                .checked_mul(record_bytes_wide)
                .ok_or(ConvertError::SizeOverflow)?,
            len: (self.point_count as u64)
                .checked_mul(record_bytes_wide)
                .ok_or(ConvertError::SizeOverflow)?,
        };
        let mut reader = store.reader(self.spool, extent)?;
        let record_bytes = self.layout.record_bytes();
        let batch_capacity = (SPOOL_IO_BUFFER_BYTES / record_bytes).max(1);
        let mut record_buffer = vec![0; batch_capacity * record_bytes];
        let mut positions = Vec::with_capacity(self.point_count);
        let mut colors = Vec::with_capacity(self.point_count);
        let mut intensities = Vec::with_capacity(self.point_count);
        let mut classifications = Vec::with_capacity(self.point_count);
        let mut return_numbers = Vec::with_capacity(self.point_count);
        let mut returns_per_pulse = Vec::with_capacity(self.point_count);
        let mut scan_angles = Vec::with_capacity(self.point_count);
        let mut user_data = Vec::with_capacity(self.point_count);
        let mut point_source_ids = Vec::with_capacity(self.point_count);
        let mut gps_times = Vec::with_capacity(if self.layout.has_gps_time {
            self.point_count
        } else {
            0
        });
        let mut rgb_values = Vec::with_capacity(if self.layout.has_color {
            self.point_count.saturating_mul(3)
        } else {
            0
        });
        let mut nir_values = Vec::with_capacity(if self.layout.has_nir {
            self.point_count
        } else {
            0
        });
        let mut las_flags = Vec::with_capacity(if self.layout.has_flags {
            self.point_count
        } else {
            0
        });
        let mut extra_values: Vec<_> = self
            .extras
            .iter()
            .map(|extra| extra.new_values(self.point_count))
            .collect();
        let mut remaining = self.point_count;
        while remaining > 0 {
            let requested = remaining.min(batch_capacity);
            let buffer_bytes = requested * record_bytes;
            let records =
                read_records(&mut reader, &mut record_buffer[..buffer_bytes], self.layout)?;
            if records != requested {
                return Err(ConvertError::PointCountMismatch {
                    expected: self.point_count as u64,
                    actual: (self.point_count - remaining + records) as u64,
                });
            }
            for bytes in record_buffer[..buffer_bytes].chunks_exact(record_bytes) {
                let point = decode_point(bytes, self.layout);
                positions.push(std::array::from_fn(|axis| {
                    point.position[axis] - frame.origin()[axis]
                }));
                colors.push(point.display_color);
                intensities.push(point.intensity);
                classifications.push(point.classification);
                return_numbers.push(point.return_number);
                returns_per_pulse.push(point.returns_per_pulse);
                scan_angles.push(point.scan_angle);
                user_data.push(point.user_data);
                point_source_ids.push(point.point_source_id);
                if self.layout.has_gps_time {
                    gps_times.push(point.gps_time);
                }
                if self.layout.has_color {
                    rgb_values.extend_from_slice(&point.rgb);
                }
                if self.layout.has_nir {
                    nir_values.push(point.nir);
                }
                if self.layout.has_flags {
                    las_flags.push(point.las_flags);
                }
                let mut offset = 0;
                for (extra, values) in self.extras.iter().zip(&mut extra_values) {
                    let end = offset + extra.byte_len();
                    extra.push_values(&point.extra[offset..end], values);
                    offset = end;
                }
            }
            remaining -= records;
        }

        let mut descriptors =
            viewer_core::las::las_attribute_schema_for(&viewer_core::las::LasSchemaSpec {
                has_gps_time: self.layout.has_gps_time,
                has_color: self.layout.has_color,
                has_nir: self.layout.has_nir,
                las_flags: self.layout.has_flags,
                extra_bytes: &self.extras,
            })?
            .into_iter();
        let mut attributes = Vec::with_capacity(10);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "intensity"),
            AttributeValues::U16(intensities),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "classification"),
            AttributeValues::U8(classifications),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "return_number"),
            AttributeValues::U8(return_numbers),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "number_of_returns"),
            AttributeValues::U8(returns_per_pulse),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "scan_angle_degrees"),
            AttributeValues::F32(scan_angles),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "user_data"),
            AttributeValues::U8(user_data),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "point_source_id"),
            AttributeValues::U16(point_source_ids),
            self.point_count,
        )?);
        if self.layout.has_gps_time {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "gps_time"),
                AttributeValues::F64(gps_times),
                self.point_count,
            )?);
        }
        if self.layout.has_color {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "rgb"),
                AttributeValues::U16(rgb_values),
                self.point_count,
            )?);
        }
        if self.layout.has_nir {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "nir"),
                AttributeValues::U16(nir_values),
                self.point_count,
            )?);
        }
        if self.layout.has_flags {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, viewer_core::las::extra::LAS_FLAGS_NAME),
                AttributeValues::U8(las_flags),
                self.point_count,
            )?);
        }
        for values in extra_values {
            let descriptor = descriptors
                .next()
                .expect("LAS attribute schema covers every column");
            attributes.push(AttributeColumn::new(descriptor, values, self.point_count)?);
        }
        Ok(SourcePointBlock::new(
            self.id, positions, colors, attributes,
        ))
    }
}
