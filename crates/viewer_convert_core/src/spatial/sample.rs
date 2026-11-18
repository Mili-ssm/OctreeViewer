//! Sample accumulation for LOD points.

use viewer_core::ColorRgb;

use super::spool::{SpoolLayout, SpoolPoint, decode_point};

/// Fixed-size source records of the nearest points of a sample grid, indexed by slot: a cell
/// keeps a `u32` slot instead of a decoded `SpoolPoint` (about 180 bytes with its Extra Bytes
/// array), which keeps accumulators small and avoids decoding and copying a full point per
/// candidate.
pub struct NearestRecords {
    record_bytes: usize,
    bytes: Vec<u8>,
}

impl NearestRecords {
    pub fn new(layout: SpoolLayout, reserve_records: usize) -> Self {
        let record_bytes = layout.record_bytes();
        Self {
            record_bytes,
            bytes: Vec::with_capacity(reserve_records * record_bytes),
        }
    }

    fn push(&mut self, record: &[u8]) -> u32 {
        let slot = (self.bytes.len() / self.record_bytes) as u32;
        self.bytes.extend_from_slice(record);
        slot
    }

    fn set(&mut self, slot: u32, record: &[u8]) {
        let start = slot as usize * self.record_bytes;
        self.bytes[start..start + self.record_bytes].copy_from_slice(record);
    }

    fn get(&self, slot: u32) -> &[u8] {
        let start = slot as usize * self.record_bytes;
        &self.bytes[start..start + self.record_bytes]
    }
}

pub struct SampleAccumulator {
    count: u64,
    // `u64` sums hold `count * 65535` while `count < 2^48`, far above any point cloud; `u128`
    // would double the size of every occupied cell of a large sample grid.
    display_color: [u64; 3],
    rgb: [u64; 3],
    nearest_distance_squared: f64,
    /// Slot of the nearest record in the grid's [`NearestRecords`]; `NO_SLOT` before the first.
    slot: u32,
}

impl Default for SampleAccumulator {
    fn default() -> Self {
        Self {
            count: 0,
            display_color: [0; 3],
            rgb: [0; 3],
            nearest_distance_squared: 0.0,
            slot: Self::NO_SLOT,
        }
    }
}

impl SampleAccumulator {
    const NO_SLOT: u32 = u32::MAX;

    /// Adds the source record `record` (of `layout`) at `position`, which must be the position
    /// `decode_position` returns for it.
    #[inline]
    pub fn add(
        &mut self,
        records: &mut NearestRecords,
        layout: SpoolLayout,
        record: &[u8],
        position: [f64; 3],
        center: [f64; 3],
    ) {
        self.count += 1;
        let tail = record.len() - 3;
        for axis in 0..3 {
            self.display_color[axis] += u64::from(record[tail + axis]);
        }
        if layout.has_color {
            let offset = layout.rgb_offset();
            for axis in 0..3 {
                self.rgb[axis] += u64::from(u16::from_le_bytes([
                    record[offset + 2 * axis],
                    record[offset + 2 * axis + 1],
                ]));
            }
        }
        let delta_x = position[0] - center[0];
        let delta_y = position[1] - center[1];
        let delta_z = position[2] - center[2];
        let distance_squared = delta_x * delta_x + delta_y * delta_y + delta_z * delta_z;
        if self.slot == Self::NO_SLOT {
            self.slot = records.push(record);
            self.nearest_distance_squared = distance_squared;
        } else if distance_squared < self.nearest_distance_squared {
            self.nearest_distance_squared = distance_squared;
            records.set(self.slot, record);
        }
    }

    /// Folds in the accumulator of the points that follow this one's in the source order. The
    /// later nearest point wins only when strictly closer, as in `add`, so ties keep the earlier.
    pub fn merge(
        &mut self,
        records: &mut NearestRecords,
        later: &Self,
        later_records: &NearestRecords,
    ) {
        self.count += later.count;
        for axis in 0..3 {
            self.display_color[axis] += later.display_color[axis];
            self.rgb[axis] += later.rgb[axis];
        }
        if later.slot == Self::NO_SLOT {
            return;
        }
        let record = later_records.get(later.slot);
        if self.slot == Self::NO_SLOT {
            self.slot = records.push(record);
            self.nearest_distance_squared = later.nearest_distance_squared;
        } else if later.nearest_distance_squared < self.nearest_distance_squared {
            self.nearest_distance_squared = later.nearest_distance_squared;
            records.set(self.slot, record);
        }
    }

    fn nearest(&self, records: &NearestRecords, layout: SpoolLayout) -> SpoolPoint {
        assert_ne!(self.slot, Self::NO_SLOT, "sample has a source point");
        decode_point(records.get(self.slot), layout)
    }

    /// Sample placed at its nearest source point instead of the cell center: used where the cell
    /// is so large (`a < 32`) that a center far from the data could not be held in `f32`.
    pub fn finish_at_nearest(&self, records: &NearestRecords, layout: SpoolLayout) -> SpoolPoint {
        let nearest = self.nearest(records, layout);
        self.finish(records, layout, nearest.position)
    }

    pub fn finish(
        &self,
        records: &NearestRecords,
        layout: SpoolLayout,
        center: [f64; 3],
    ) -> SpoolPoint {
        let mut point = self.nearest(records, layout);
        point.position = center;
        point.display_color = ColorRgb::new(
            average(self.display_color[0], self.count) as u8,
            average(self.display_color[1], self.count) as u8,
            average(self.display_color[2], self.count) as u8,
        );
        point.rgb = std::array::from_fn(|axis| average(self.rgb[axis], self.count) as u16);
        point
    }
}

#[inline]
fn average(sum: u64, count: u64) -> u64 {
    (sum + count / 2) / count
}
