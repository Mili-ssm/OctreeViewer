//! Moved to `viewer_convert_core`; re-exported under the old path.

pub(crate) use viewer_convert_core::pipeline::point_source::*;

#[cfg(test)]
mod tests {
    use std::{fs, io::Read, path::Path};

    use las::{Builder, Writer, point::Format};

    use super::{LasPointSource, PointRange, PointSource};
    use crate::pipeline::{
        node_job::spool_source,
        spool_store::{FileSpoolStore, SpoolExtent, SpoolId, SpoolStore},
    };
    use crate::{
        ConvertError,
        las_source::{inspect_source, open_las_reader},
        options::input_batch_point_limit,
    };

    const POINTS: u64 = 1000;
    const BATCH: u64 = 64;

    fn write_las(path: &Path) {
        let mut builder = Builder::from((1, 2));
        builder.point_format = Format::new(0).unwrap();
        let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
        for index in 0..POINTS {
            writer
                .write_point(las::Point {
                    x: 1_000_000.0 + index as f64 * 0.25,
                    y: 2_000_000.0 + (index % 3) as f64,
                    z: 50.0 + (index % 5) as f64,
                    ..las::Point::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
    }

    /// Spools the file either serially (`parts == None`) or by record ranges, one file per
    /// range; returns the concatenated bytes, or the error of the lowest failing range.
    fn spool(path: &Path, parts: Option<usize>) -> Result<Vec<u8>, ConvertError> {
        spool_ending_at(path, parts, u64::MAX)
    }

    /// Decorator that makes the file "end" at global point `end`, as a truncated input would.
    /// `open_las_reader` preflights the layout, so a real truncated LAS never reaches `fill`.
    struct EndsAt<P>(P, u64);

    impl<P: PointSource> PointSource for EndsAt<P> {
        fn header(&self) -> &las::Header {
            self.0.header()
        }
        fn range(&self) -> PointRange {
            self.0.range()
        }
        fn next_index(&self) -> u64 {
            self.0.next_index()
        }
        fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError> {
            self.0
                .fill(count.min(self.1.saturating_sub(self.0.next_index())), data)
        }
    }

    fn spool_ending_at(
        path: &Path,
        parts: Option<usize>,
        end: u64,
    ) -> Result<Vec<u8>, ConvertError> {
        let reader = open_las_reader(path).unwrap();
        let info = inspect_source(&reader).unwrap();
        let point_limit = input_batch_point_limit(BATCH, info.record_length);
        let ranges = match parts {
            None => vec![None],
            Some(parts) => PointRange::split(info.point_count, BATCH, parts)
                .into_iter()
                .map(Some)
                .collect(),
        };
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let mut bytes = Vec::new();
        for (index, range) in ranges.into_iter().enumerate() {
            let id = SpoolId(index as u64);
            let reader = open_las_reader(path).unwrap();
            let mut progress = |_| Ok(());
            match range {
                None => spool_source(
                    &mut EndsAt(LasPointSource::full(reader), end),
                    &info,
                    point_limit,
                    &store,
                    id,
                    &mut progress,
                )?,
                Some(range) => spool_source(
                    &mut EndsAt(LasPointSource::records(reader, range)?, end),
                    &info,
                    point_limit,
                    &store,
                    id,
                    &mut progress,
                )?,
            };
            let extent = SpoolExtent {
                offset: 0,
                len: store.len(id).unwrap(),
            };
            store
                .reader(id, extent)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
        }
        Ok(bytes)
    }

    #[test]
    fn ranged_reads_cover_the_file_with_the_serial_points() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.las");
        write_las(&path);
        let ranges = PointRange::split(POINTS, BATCH, 3);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, POINTS);
        assert!(ranges.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert!(ranges.iter().all(|range| range.start % BATCH == 0));
        let serial = spool(&path, None).unwrap();
        assert!(!serial.is_empty());
        assert_eq!(spool(&path, Some(3)).unwrap(), serial);
    }

    #[test]
    fn early_end_of_points_fails_identically_serially_and_by_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.las");
        write_las(&path);
        let serial = spool_ending_at(&path, None, 750).unwrap_err();
        let ranged = spool_ending_at(&path, Some(3), 750).unwrap_err();
        assert!(
            matches!(
                serial,
                ConvertError::PointCountMismatch {
                    expected: 1000,
                    actual: 750
                }
            ),
            "{serial}"
        );
        assert_eq!(serial.to_string(), ranged.to_string());
    }

    #[test]
    fn out_of_bounds_point_reports_the_same_global_index_by_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.las");
        write_las(&path);
        // Header max X lowered so the first offending point (index 601) is in the second range.
        let mut file = fs::read(&path).unwrap();
        file[179..187].copy_from_slice(&1_000_150.0_f64.to_le_bytes());
        fs::write(&path, file).unwrap();
        let serial = spool(&path, None).unwrap_err();
        let ranged = spool(&path, Some(3)).unwrap_err();
        assert!(
            matches!(
                serial,
                ConvertError::PointOutsideHeaderBounds {
                    point_index: 601,
                    ..
                }
            ),
            "{serial}"
        );
        assert_eq!(serial.to_string(), ranged.to_string());
    }

    #[test]
    fn decoder_panic_carries_the_global_point_index() {
        let error =
            crate::las_source::contain_decoder_panic_at(Some(7), || panic!("boom")).unwrap_err();
        assert!(matches!(
            error,
            ConvertError::LasDecoderPanicked {
                point_index: Some(7),
                ..
            }
        ));
    }
}
