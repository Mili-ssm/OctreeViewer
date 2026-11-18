//! Reading source points into the root spool.

use std::io::Write;

use las::PointData;
use viewer_core::las::{extra::MAX_EXTRA_BYTES_PER_POINT, las_flags_of_record};
use viewer_core::{ColorRgb, SpatialBounds};

use super::spool::{CHILD_SPOOL_BUFFER_BYTES, SpoolLayout, SpoolPoint, write_point};
use crate::ConvertError;
use crate::report::{ConversionProgress, ConversionStage, emit_progress};
use crate::source::SourceInfo;

const HEADER_BOUNDS_RELATIVE_TOLERANCE: f64 = 1.0e-7;
const HEADER_BOUNDS_MAX_TOLERANCE: f64 = 0.5;

/// Forwards `JobProgress::SourcePoints` of a job reading the source to the host callback.
pub fn emit_source_progress(
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    total_points: u64,
    processed: u64,
) -> Result<(), ConvertError> {
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::PlanningSpatialIndex,
            points_processed: processed,
            total_points,
            bytes_written: 0,
        },
        processed,
    )
}

pub fn write_source_points(
    data: &PointData,
    source: &SourceInfo,
    processed: u64,
    writer: &mut impl Write,
) -> Result<(), ConvertError> {
    let layout = SpoolLayout::source(source);
    let mut encoded = Vec::with_capacity(CHILD_SPOOL_BUFFER_BYTES);
    visit_source_points(data, source, processed, |point| {
        if encoded.len() + layout.record_bytes() > CHILD_SPOOL_BUFFER_BYTES {
            writer.write_all(&encoded)?;
            encoded.clear();
        }
        write_point(&mut encoded, &point, layout).map_err(ConvertError::Io)
    })?;
    if !encoded.is_empty() {
        writer.write_all(&encoded)?;
    }
    Ok(())
}

pub fn visit_source_points(
    data: &PointData,
    source: &SourceInfo,
    processed: u64,
    mut visitor: impl FnMut(SpoolPoint) -> Result<(), ConvertError>,
) -> Result<(), ConvertError> {
    let mut intensities = data.intensity();
    let mut classifications = data.classification();
    let mut return_numbers = data.return_number();
    let mut returns_per_pulse = data.number_of_returns();
    let mut scan_angles = data.scan_angle_degrees();
    let mut user_data = data.user_data();
    let mut point_source_ids = data.point_source_id();
    let mut gps_times = data.gps_time();
    let mut nir_values = data.nir();
    let mut rgb_values = data.rgb();
    if source.point_format.has_gps_time != gps_times.is_some()
        || source.point_format.has_nir != nir_values.is_some()
        || source.point_format.has_color != rgb_values.is_some()
    {
        return Err(ConvertError::InvalidAttributeSchema);
    }
    let layout = SpoolLayout::source(source);
    let records = data.raw_bytes().chunks_exact(data.record_len());
    for (index, (((x, y), z), record)) in data
        .x_raw()
        .zip(data.y_raw())
        .zip(data.z_raw())
        .zip(records)
        .enumerate()
    {
        let raw_position = [x, y, z];
        let exact = layout.exact_position(raw_position);
        let position = layout.position(raw_position);
        let local = std::array::from_fn(|axis| position[axis] - source.frame.origin()[axis]);
        if position.iter().any(|coordinate| !coordinate.is_finite())
            || local.iter().any(|coordinate| !coordinate.is_finite())
        {
            return Err(ConvertError::CoordinateFrame(
                viewer_core::CoordinateFrameError::PositionOutOfRange(local),
            ));
        }
        if outside_header_bounds(exact, source.bounds) {
            return Err(ConvertError::PointOutsideHeaderBounds {
                point_index: processed + index as u64,
                position: exact,
            });
        }
        let intensity = intensities
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let classification = classifications
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let return_number = return_numbers
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let returns_per_pulse = returns_per_pulse
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let scan_angle = scan_angles
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let user_data = user_data
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let point_source_id = point_source_ids
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let gps_time = gps_times
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .unwrap_or(0.0);
        let nir = nir_values
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .unwrap_or(0);
        let rgb = rgb_values
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .map_or_default(|(red, green, blue)| [red, green, blue]);
        let display_color = if source.point_format.has_color {
            ColorRgb::new(
                (rgb[0] >> 8) as u8,
                (rgb[1] >> 8) as u8,
                (rgb[2] >> 8) as u8,
            )
        } else {
            let gray = (intensity >> 8) as u8;
            ColorRgb::new(gray, gray, gray)
        };
        let mut extra = [0; MAX_EXTRA_BYTES_PER_POINT];
        let extra_len = usize::from(layout.extra_len);
        let las_flags = if layout.has_flags {
            extra[..extra_len].copy_from_slice(&record[record.len() - extra_len..]);
            las_flags_of_record(record, layout.is_extended)
        } else {
            0
        };
        visitor(SpoolPoint {
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
        })?;
    }
    if intensities.next().is_some()
        || classifications.next().is_some()
        || return_numbers.next().is_some()
        || returns_per_pulse.next().is_some()
        || scan_angles.next().is_some()
        || user_data.next().is_some()
        || point_source_ids.next().is_some()
        || gps_times
            .as_mut()
            .is_some_and(|values| values.next().is_some())
        || nir_values
            .as_mut()
            .is_some_and(|values| values.next().is_some())
        || rgb_values
            .as_mut()
            .is_some_and(|values| values.next().is_some())
    {
        return Err(ConvertError::InvalidAttributeSchema);
    }
    Ok(())
}

#[inline]
pub(super) fn outside_header_bounds(position: [f64; 3], bounds: SpatialBounds) -> bool {
    (0..3).any(|axis| {
        let min = bounds.min()[axis];
        let max = bounds.max()[axis];
        let coordinate_magnitude = position[axis].abs().max(min.abs()).max(max.abs());
        let tolerance = (coordinate_magnitude * HEADER_BOUNDS_RELATIVE_TOLERANCE)
            .min(HEADER_BOUNDS_MAX_TOLERANCE);
        position[axis] < min - tolerance || position[axis] > max + tolerance
    })
}
