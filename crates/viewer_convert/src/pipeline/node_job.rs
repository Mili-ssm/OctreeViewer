//! Moved to `viewer_convert_core`; re-exported under the old path.

pub(crate) use viewer_convert_core::pipeline::node_job::*;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use las::{Builder, Writer};
    use viewer_core::SpatialBounds;

    use super::super::spool_store::{SpoolId, SpoolStore};
    use super::*;
    use crate::las_source::{inspect_source, open_las_reader};
    use crate::pipeline::{
        clock::SystemClock,
        point_source::LasPointSource,
        spool_store::{MemSpoolStore, SpoolAccounting},
    };
    use crate::source::SourceInfo;
    use crate::spatial::SpoolLayout;

    const LIMIT: u64 = 16;

    fn write_las(path: &Path, points: &[[f64; 3]]) {
        let header = Builder::from((1, 2)).into_header().unwrap();
        let mut writer = Writer::from_path(path, header).unwrap();
        for (index, [x, y, z]) in points.iter().copied().enumerate() {
            writer
                .write_point(las::Point {
                    x,
                    y,
                    z,
                    intensity: index as u16,
                    ..las::Point::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
    }

    fn context(info: &SourceInfo, divisions: u8) -> JobContext {
        JobContext {
            point_limit: LIMIT,
            spatial_divisions: divisions,
            sample_grid: viewer_convert_core::options::DEFAULT_SAMPLE_GRID,
            frame: info.frame,
            layout: SpoolLayout::source(info),
            collect_diagnostics: true,
        }
    }

    fn cube(min: f64, max: f64) -> SpatialBounds {
        SpatialBounds::new([min; 3], [max; 3]).unwrap()
    }

    /// Spools `points` into `SOURCE` of a fresh in-memory store.
    fn spooled(dir: &Path, points: &[[f64; 3]]) -> (SourceInfo, MemSpoolStore) {
        let path = dir.join("input.las");
        write_las(&path, points);
        let reader = open_las_reader(&path).unwrap();
        let info = inspect_source(&reader).unwrap();
        let store = MemSpoolStore::new(false);
        spool_source(
            &mut LasPointSource::full(reader),
            &info,
            LIMIT,
            &store,
            SpoolId::SOURCE,
            &mut |_| Ok(()),
        )
        .unwrap();
        (info, store)
    }

    fn job(info: &SourceInfo, bounds: SpatialBounds) -> NodeJob {
        NodeJob {
            path: NodePath(vec![3]),
            level: 1,
            bounds,
            input: SpoolId::SOURCE,
            point_count: info.point_count,
            input_is_child_spool: true,
            first_output: SpoolId(100),
        }
    }

    #[test]
    fn node_paths_order_as_preorder() {
        let mut paths = [vec![1], vec![0, 7], vec![], vec![0], vec![1, 0]]
            .map(NodePath)
            .to_vec();
        paths.sort();
        assert_eq!(
            paths,
            [vec![], vec![0], vec![0, 7], vec![1], vec![1, 0]].map(NodePath)
        );
    }

    #[test]
    fn empty_children_create_no_spool_and_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // Two opposite octants of a divisions-2 node; the other six stay empty.
        let points = (0..40)
            .map(|i| {
                let offset = f64::from(i % 20) * 0.1;
                if i < 20 {
                    [1.0 + offset; 3]
                } else {
                    [7.0 + offset; 3]
                }
            })
            .collect::<Vec<_>>();
        let (info, store) = spooled(dir.path(), &points);
        let output = run_node_job(
            &context(&info, 2),
            &job(&info, cube(0.0, 10.0)),
            &store,
            &SystemClock,
            &mut |_| Ok(()),
        )
        .unwrap();

        let keys = output.children.iter().map(|c| c.key).collect::<Vec<_>>();
        assert_eq!(keys, [0, 7]);
        assert_eq!(output.children[0].spool, SpoolId(100));
        assert_eq!(output.children[1].spool, SpoolId(107));
        assert!(output.children.iter().all(|c| c.point_count == 20));
        for key in 1..7 {
            assert!(store.len(SpoolId(100 + key)).is_err(), "key {key}");
        }
        assert_eq!(output.release, [SpoolId::SOURCE]);
        assert_eq!(output.path, NodePath(vec![3]));
        let lod = &output.blocks[..];
        assert_eq!(lod.len(), 1);
        assert_eq!(
            lod[0].spool,
            SpoolId(108),
            "LOD id is first_output + divisions³"
        );
        assert!(!lod[0].is_source_points && lod[0].final_block_in_spool);
        assert_eq!(output.point_count, lod[0].point_count as u64);
        let metrics = output.metrics.unwrap();
        assert_eq!(metrics.child_spool_bytes.len(), 2);
        assert_eq!(metrics.scanned_bytes, store.len(SpoolId::SOURCE).unwrap());
        assert!(!metrics.became_leaf_all_equal);
    }

    #[test]
    fn a_node_with_all_points_equal_becomes_a_leaf_and_releases_its_children() {
        let dir = tempfile::tempdir().unwrap();
        // Enough records for one 32 KiB flush, so a child spool is really created.
        let (info, store) = spooled(dir.path(), &[[2.0, 3.0, 4.0]; 2_000]);
        let output = run_node_job(
            &context(&info, 2),
            &job(&info, cube(0.0, 10.0)),
            &store,
            &SystemClock,
            &mut |_| Ok(()),
        )
        .unwrap();

        assert!(output.is_leaf());
        assert_eq!(output.point_count, 2_000);
        assert_eq!(output.geometric_error, 0.0);
        assert_eq!(output.release, [SpoolId(100)], "the only created child");
        assert!(output.metrics.as_ref().unwrap().became_leaf_all_equal);
        assert_eq!(output.blocks.len(), 2_000_usize.div_ceil(LIMIT as usize));
        assert!(output.blocks.iter().all(|b| b.spool == SpoolId::SOURCE));
        let finals = output.blocks.iter().filter(|b| b.final_block_in_spool);
        assert_eq!(finals.count(), 1);
        assert!(output.blocks.last().unwrap().final_block_in_spool);
        // Released children are closed (the store rejects deleting an open spool); the input is kept.
        store.delete(SpoolId(100)).unwrap();
        assert_eq!(store.stats().live_spools, 1);
        assert!(store.len(SpoolId::SOURCE).is_ok());
    }

    #[test]
    fn an_all_equal_root_closes_its_children_unflushed_and_releases_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.las");
        write_las(&path, &[[2.0, 3.0, 4.0]; 2_000]);
        let reader = open_las_reader(&path).unwrap();
        let info = inspect_source(&reader).unwrap();
        let store = MemSpoolStore::new(true);
        let mut seen = Vec::new();
        let partition = run_root_partition(
            &context(&info, 2),
            &info,
            cube(0.0, 10.0),
            SpoolId(1),
            &mut LasPointSource::full(reader),
            &store,
            &SystemClock,
            &mut |progress| {
                if let JobProgress::SourcePoints(points) = progress {
                    seen.push(points);
                }
                Ok(())
            },
        )
        .unwrap();

        assert!(partition.all_points_equal);
        assert!(partition.children.is_empty());
        assert_eq!(partition.release, [SpoolId(1)]);
        assert_eq!(seen.last(), Some(&2_000));
        let flushed = partition.metrics.unwrap().child_spool_bytes;
        assert_eq!(flushed.len(), 1);
        // Only the flushed 32 KiB chunk reached the store; the buffered tail was dropped.
        assert_eq!(store.len(SpoolId(1)).unwrap(), flushed[0]);
        assert!(flushed[0] < 2_000 * SpoolLayout::source(&info).record_bytes() as u64);
        store.delete(SpoolId(1)).unwrap();
        assert_eq!(store.stats().live_spools, 0);
        assert!(store.ledger_totals().is_some());
    }
}
