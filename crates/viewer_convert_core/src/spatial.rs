mod block;
mod builder;
#[doc(hidden)]
pub mod estimate;
#[doc(hidden)]
pub mod executor;
mod geometry;
mod memory;
#[doc(hidden)]
pub mod nn_distance;
#[doc(hidden)]
pub mod node_job;
mod plan;
mod sample;
mod source_points;
mod spool;

pub use block::PlannedBlock;
pub use builder::{BranchOutput, OpenNodeEvent, SpatialBuilder};
pub use geometry::{
    GlobalCellFrame, center_rounding_error, cube_side, global_level, lod_sample_error,
    sample_cell_global, voxel_center_global,
};
pub use plan::{SpatialPlan, build_spatial_plan_from_source};
pub use sample::{NearestRecords, SampleAccumulator};
pub use source_points::{emit_source_progress, visit_source_points, write_source_points};
pub(crate) use spool::MAX_SPOOL_RECORD_BYTES;
pub use spool::{
    ChildSpool, SpoolLayout, SpoolPoint, StoreWriter, decode_point, decode_position, read_records,
    write_point,
};

#[cfg(test)]
use {
    crate::pipeline::spool_store::{SpoolId, SpoolStore},
    crate::source::SourceInfo,
    geometry::{child_bounds, child_index},
    memory::child_spool_buffer_bytes,
    source_points::outside_header_bounds,
    spool::read_point,
    std::io,
    viewer_core::las::extra::MAX_EXTRA_BYTES_PER_POINT,
    viewer_core::{AttributeValues, BlockId, ColorRgb, SpatialBounds},
};

pub const MAX_SPATIAL_DEPTH: u32 = 32;

/// Deepest job depth `consume_rank` accepts. Global-frame layouts split up to absolute level
/// `GLOBAL_MAX_INTERNAL_LEVEL`, deeper than the legacy `MAX_SPATIAL_DEPTH`; the headroom keeps
/// `(MAX_CONSUME_DEPTH + 1) << 16` inside `u32`.
pub const MAX_CONSUME_DEPTH: u32 = 64;
const _: () = assert!(MAX_CONSUME_DEPTH > node_job::GLOBAL_MAX_INTERNAL_LEVEL);
const _: () = assert!(MAX_CONSUME_DEPTH > MAX_SPATIAL_DEPTH);

/// Consume-order rank of the child spool `key` at `depth` (>= 1). The depth-first planner
/// consumes deeper spools first and, at equal depth, the lower key first; the queued spools of
/// one depth always share a parent, so `key` alone orders them.
pub fn consume_rank(depth: u32, key: u16) -> u32 {
    ((MAX_CONSUME_DEPTH + 1 - depth) << 16) | u32::from(key)
}

#[cfg(test)]
mod partition_tests {
    use super::*;
    use crate::pipeline::spool_store::MemSpoolStore;
    use las::PointDataBuilder;

    fn spool_fixture(format_id: u8) -> (SourceInfo, SpoolPoint) {
        let transforms = las::Vector {
            x: las::Transform {
                scale: 0.000_003_7,
                offset: 1.0e16,
            },
            y: las::Transform {
                scale: 0.031_415_926_535,
                offset: -9.0e14,
            },
            z: las::Transform {
                scale: 0.000_123_456_789,
                offset: 8.0e12,
            },
        };
        let raw_position = [i32::MAX - 19, i32::MIN + 31, -123_456_789];
        let position = [
            transforms.x.direct(raw_position[0]),
            transforms.y.direct(raw_position[1]),
            transforms.z.direct(raw_position[2]),
        ];
        let source = SourceInfo {
            bounds: SpatialBounds::new(position, position).unwrap(),
            frame: viewer_core::CoordinateFrame::new(position).unwrap(),
            point_count: 1,
            record_length: las::point::Format::new(format_id).unwrap().len() as usize,
            point_format: las::point::Format::new(format_id).unwrap(),
            transforms,
            has_crs: false,
            crs: viewer_format::CrsRecord::local(),
            scale_offset: None,
            global: crate::source::GlobalRoot {
                level: 0,
                prefix: [0; 3],
                frame_origin_quanta: [0; 3],
                data_max_quanta: [0; 3],
                lattice_base: [0; 3],
            },
            extra_byte_vlrs: Vec::new(),
            las_extra_bytes: None,
        };
        let point = SpoolPoint {
            raw_position,
            position,
            intensity: 45_678,
            classification: 7,
            return_number: 2,
            returns_per_pulse: 3,
            scan_angle: -12.5,
            user_data: 123,
            point_source_id: 12_345,
            gps_time: 123_456.789,
            rgb: [12_345, 23_456, 34_567],
            nir: 54_321,
            las_flags: 0,
            extra: [0; MAX_EXTRA_BYTES_PER_POINT],
            display_color: ColorRgb::new(71, 82, 93),
        };
        (source, point)
    }

    #[test]
    fn source_spools_preserve_exact_raw_xyz_with_large_origins() {
        let (source, fixture) = spool_fixture(3);
        let layout = SpoolLayout::source(&source);
        let mut las_bytes = vec![0; source.record_length];
        for (axis, coordinate) in fixture.raw_position.iter().enumerate() {
            las_bytes[axis * 4..axis * 4 + 4].copy_from_slice(&coordinate.to_le_bytes());
        }
        let data = PointDataBuilder::new()
            .with_format(source.point_format)
            .with_transforms(source.transforms)
            .build_from_bytes(las_bytes)
            .unwrap();
        let expected_position = [
            data.x().next().unwrap(),
            data.y().next().unwrap(),
            data.z().next().unwrap(),
        ];
        // The large origin loses low bits in f64; an inverse transform cannot recover the raw value.
        assert_ne!(
            source.transforms.x.inverse(expected_position[0]).ok(),
            Some(fixture.raw_position[0])
        );
        let mut bytes = Vec::new();
        write_source_points(&data, &source, 0, &mut bytes).unwrap();
        assert_eq!(bytes.len(), 41);
        assert_eq!(
            &bytes[..12],
            &fixture
                .raw_position
                .iter()
                .flat_map(|coordinate| coordinate.to_le_bytes())
                .collect::<Vec<_>>()[..]
        );
        let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
        assert_eq!(decoded.raw_position, fixture.raw_position);
        assert_eq!(
            decoded.position.map(f64::to_bits),
            expected_position.map(f64::to_bits)
        );
        let mut rewritten = Vec::new();
        write_point(&mut rewritten, &decoded, layout).unwrap();
        assert_eq!(bytes, rewritten);
    }

    #[test]
    fn child_spool_diagnostics_count_only_flushed_payload() {
        let (source, point) = spool_fixture(3);
        let layout = SpoolLayout::source(&source);
        let store = MemSpoolStore::new(true);
        let mut child = ChildSpool::new(SpoolId(1), layout, true);

        child.write_point(&store, &point).unwrap();
        assert_eq!(child.payload_written_bytes, Some(0));
        assert!(!child.created, "an unflushed child has no spool");
        child.flush(&store).unwrap();
        assert_eq!(
            child.payload_written_bytes,
            Some(layout.record_bytes() as u64)
        );
    }

    #[test]
    fn compact_layout_sizes_and_optional_attributes_match_source_format() {
        for (format_id, source_bytes, lod_bytes) in
            [(0, 27, 39), (2, 33, 45), (3, 41, 53), (8, 43, 55)]
        {
            let (source, fixture) = spool_fixture(format_id);
            let source_layout = SpoolLayout::source(&source);
            for (layout, expected_bytes) in [
                (source_layout, source_bytes),
                (source_layout.lod(), lod_bytes),
            ] {
                let mut bytes = Vec::new();
                write_point(&mut bytes, &fixture, layout).unwrap();
                assert_eq!(layout.record_bytes(), expected_bytes);
                assert_eq!(bytes.len(), expected_bytes);
                let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
                assert_eq!(decoded.intensity, fixture.intensity);
                assert_eq!(decoded.classification, fixture.classification);
                assert_eq!(decoded.return_number, fixture.return_number);
                assert_eq!(decoded.returns_per_pulse, fixture.returns_per_pulse);
                assert_eq!(decoded.scan_angle, fixture.scan_angle);
                assert_eq!(decoded.user_data, fixture.user_data);
                assert_eq!(decoded.point_source_id, fixture.point_source_id);
                assert_eq!(decoded.display_color, fixture.display_color);
                assert_eq!(
                    decoded.gps_time,
                    if layout.has_gps_time {
                        fixture.gps_time
                    } else {
                        0.0
                    }
                );
                assert_eq!(
                    decoded.rgb,
                    if layout.has_color {
                        fixture.rgb
                    } else {
                        [0; 3]
                    }
                );
                assert_eq!(decoded.nir, if layout.has_nir { fixture.nir } else { 0 });
            }
        }
    }

    #[test]
    fn synthetic_lod_centers_preserve_f64_bits_outside_las_grid() {
        let (source, fixture) = spool_fixture(3);
        let source_layout = SpoolLayout::source(&source);
        let layout = source_layout.lod();
        let center = [
            0.123_456_789_012_345,
            -1.234_567_890_123_456,
            987_654.321_098_765_4,
        ];
        let mut record = Vec::new();
        write_point(&mut record, &fixture, source_layout).unwrap();
        let mut records = NearestRecords::new(source_layout, 1);
        let mut sample = SampleAccumulator::default();
        sample.add(
            &mut records,
            source_layout,
            &record,
            fixture.position,
            center,
        );
        let synthetic = sample.finish(&records, source_layout, center);
        let mut bytes = Vec::new();
        write_point(&mut bytes, &synthetic, layout).unwrap();
        assert_eq!(bytes.len(), 53);
        let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
        assert_eq!(decoded.position.map(f64::to_bits), center.map(f64::to_bits));
        assert_eq!(decoded.gps_time, fixture.gps_time);
        assert_eq!(decoded.rgb, fixture.rgb);
        assert_eq!(decoded.display_color, fixture.display_color);
    }

    #[test]
    fn planned_blocks_seek_using_their_compact_record_layout() {
        let store = MemSpoolStore::new(false);
        let mut next_id = 1;
        for format_id in [0, 2, 3, 8] {
            let (source, fixture) = spool_fixture(format_id);
            let source_layout = SpoolLayout::source(&source);
            for layout in [source_layout, source_layout.lod()] {
                let mut second = fixture;
                second.raw_position[2] += 123;
                second.position = layout.position(second.raw_position);
                second.intensity += 1;
                let spool = SpoolId(next_id);
                next_id += 1;
                let mut bytes = Vec::new();
                write_point(&mut bytes, &fixture, layout).unwrap();
                write_point(&mut bytes, &second, layout).unwrap();
                store.create(spool, 1).unwrap();
                store.append(spool, &bytes).unwrap();
                store.close(spool).unwrap();
                let block = PlannedBlock {
                    id: BlockId::new(0),
                    spool,
                    offset_points: 1,
                    point_count: 1,
                    layout,
                    extras: Vec::new().into(),
                    is_source_points: layout.source_coordinates,
                    final_block_in_spool: true,
                }
                .read(&store, source.frame)
                .unwrap();
                assert_eq!(
                    block.positions_local(),
                    &[std::array::from_fn(
                        |axis| second.position[axis] - source.frame.origin()[axis]
                    )]
                );
                assert_eq!(block.colors(), &[second.display_color]);
                assert_eq!(
                    block.attributes()[0].values(),
                    &AttributeValues::U16(vec![second.intensity])
                );
                assert_eq!(
                    block.attributes().len(),
                    7 + usize::from(layout.has_gps_time)
                        + usize::from(layout.has_color)
                        + usize::from(layout.has_nir)
                );
            }
        }
    }

    #[test]
    fn bulk_spool_reads_decode_records_and_reject_partial_tails() {
        for format_id in [0, 2, 3, 8] {
            let (source, fixture) = spool_fixture(format_id);
            let layout = SpoolLayout::source(&source);
            let mut expected = fixture;
            expected.raw_position[0] += 19;
            expected.position = layout.position(expected.raw_position);
            let mut bytes = Vec::new();
            write_point(&mut bytes, &fixture, layout).unwrap();
            write_point(&mut bytes, &expected, layout).unwrap();

            let mut reader = bytes.as_slice();
            let mut batch = vec![0; layout.record_bytes() * 3];
            assert_eq!(read_records(&mut reader, &mut batch, layout).unwrap(), 2);
            let decoded = batch
                .chunks_exact(layout.record_bytes())
                .map(|record| decode_point(record, layout))
                .collect::<Vec<_>>();
            assert_eq!(decoded[0].raw_position, fixture.raw_position);
            assert_eq!(decoded[1].raw_position, expected.raw_position);
            assert_eq!(decoded[1].intensity, expected.intensity);

            bytes.push(0);
            let mut truncated = bytes.as_slice();
            let error = read_records(&mut truncated, &mut batch, layout).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        }
    }

    #[test]
    fn header_bounds_allow_small_metadata_rounding_but_reject_real_outliers() {
        let bounds = SpatialBounds::new(
            [692_199.5, 4_285_716.0, 329.910_095_214_843_75],
            [692_838.687_5, 4_286_262.5, 359.015_899_658_203_1],
        )
        .unwrap();

        assert!(!outside_header_bounds(
            [692_399.511_1, 4_285_715.937_7, 335.174],
            bounds
        ));
        assert!(outside_header_bounds(
            [692_399.511_1, 4_285_715.0, 335.174],
            bounds
        ));
    }

    #[test]
    fn adaptive_cells_cover_centers_and_assign_internal_planes_to_upper_cell() {
        let bounds = SpatialBounds::new([-4.0, 2.0, -8.0], [4.0, 10.0, 0.0]).unwrap();
        for divisions in [2_u8, 4, 8] {
            let divisions_usize = usize::from(divisions);
            for x in 0..divisions_usize {
                for y in 0..divisions_usize {
                    for z in 0..divisions_usize {
                        let key = (x * divisions_usize + y) * divisions_usize + z;
                        let child = child_bounds(bounds, key, divisions);
                        let position = std::array::from_fn(|axis| {
                            f64::midpoint(child.min()[axis], child.max()[axis])
                        });
                        assert_eq!(child_index(position, bounds, divisions), key);
                        for axis in 0..3 {
                            assert!(position[axis] >= child.min()[axis]);
                            assert!(position[axis] <= child.max()[axis]);
                            if [x, y, z][axis] + 1 < divisions_usize {
                                let plane = child.max()[axis];
                                let mut on_plane = position;
                                on_plane[axis] = plane;
                                let next_key = child_index(on_plane, bounds, divisions);
                                let expected_cell = [x, y, z][axis] + 1;
                                let actual_cell = match axis {
                                    0 => next_key / (divisions_usize * divisions_usize),
                                    1 => (next_key / divisions_usize) % divisions_usize,
                                    _ => next_key % divisions_usize,
                                };
                                assert_eq!(actual_cell, expected_cell);
                            }
                        }
                    }
                }
            }
            assert_eq!(
                child_index(bounds.max(), bounds, divisions),
                divisions_usize.pow(3) - 1
            );
        }
    }

    #[test]
    fn binary_partition_keeps_midpoint_and_key_order() {
        let bounds = SpatialBounds::new([0.0; 3], [8.0; 3]).unwrap();
        assert_eq!(child_index([3.0; 3], bounds, 2), 0);
        assert_eq!(child_index([4.0; 3], bounds, 2), 7);
        let upper = child_bounds(bounds, 7, 2);
        assert_eq!(upper.min(), [4.0; 3]);
        assert_eq!(upper.max(), [8.0; 3]);
    }

    #[test]
    fn fanout_spool_memory_is_bounded_for_supported_divisions() {
        assert_eq!(child_spool_buffer_bytes(2).unwrap(), 8 * 32 * 1024);
        assert_eq!(child_spool_buffer_bytes(4).unwrap(), 64 * 32 * 1024);
        assert_eq!(child_spool_buffer_bytes(8).unwrap(), 512 * 32 * 1024);
    }
}
