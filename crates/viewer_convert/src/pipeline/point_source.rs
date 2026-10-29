use std::borrow::BorrowMut;

use las::Reader;

use crate::{
    ConvertError,
    las_source::{contain_decoder_panic_at, seek_points_at},
};

/// Half-open range of global point indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PointRange {
    pub start: u64,
    pub end: u64,
}

impl PointRange {
    /// Splits `total` points into at most `parts` contiguous ranges whose starts are multiples
    /// of `batch`, so each range issues the same `fill` calls as the serial read.
    #[cfg_attr(not(test), allow(dead_code, reason = "used by M2 range planning"))]
    pub fn split(total: u64, batch: u64, parts: usize) -> Vec<Self> {
        let batches = total.div_ceil(batch.max(1));
        let per_part = batches.div_ceil(parts.max(1) as u64).max(1);
        (0..batches)
            .step_by(per_part as usize)
            .map(|first| Self {
                start: first * batch,
                end: ((first + per_part) * batch).min(total),
            })
            .collect()
    }
}

pub(crate) trait PointSource {
    fn header(&self) -> &las::Header;
    fn range(&self) -> PointRange;
    /// Global index of the next point.
    fn next_index(&self) -> u64;
    /// Up to `count` points, clamped to the range end; fewer only if the file ends early.
    /// Decoder panics are contained and reported with the global index of the first point.
    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError>;
}

pub(crate) struct LasPointSource<R: BorrowMut<Reader>> {
    reader: R,
    range: PointRange,
    next: u64,
}

impl<R: BorrowMut<Reader>> LasPointSource<R> {
    /// The whole file, read from the reader's current position (start of the point data).
    pub fn full(reader: R) -> Self {
        let end = reader.borrow().header().number_of_points();
        Self {
            reader,
            range: PointRange { start: 0, end },
            next: 0,
        }
    }

    /// Seeks to `range.start` on fixed-size records; compressed input is rejected because
    /// LAZ chunks have variable size (A9).
    #[cfg_attr(not(test), allow(dead_code, reason = "used by M2 range reads"))]
    pub fn records(mut reader: R, range: PointRange) -> Result<Self, ConvertError> {
        if reader.borrow().header().point_format().is_compressed {
            return Err(ConvertError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "record ranges need fixed-size (uncompressed) LAS records",
            )));
        }
        if range.start > 0 {
            seek_points_at(reader.borrow_mut(), range.start, range.start)?;
        }
        Ok(Self {
            reader,
            range,
            next: range.start,
        })
    }
}

impl<R: BorrowMut<Reader>> PointSource for LasPointSource<R> {
    fn header(&self) -> &las::Header {
        self.reader.borrow().header()
    }

    fn range(&self) -> PointRange {
        self.range
    }

    fn next_index(&self) -> u64 {
        self.next
    }

    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError> {
        let count = count.min(self.range.end - self.next);
        let reader = self.reader.borrow_mut();
        let read = contain_decoder_panic_at(Some(self.next), || reader.fill_points(count, data))??;
        self.next += read;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read, path::Path};

    use las::{Builder, Writer, point::Format};

    use super::{LasPointSource, PointRange, PointSource};
    use crate::pipeline::spool_store::{FileSpoolStore, SpoolExtent, SpoolId, SpoolStore};
    use crate::{
        ConvertError, ConvertOptions,
        las_source::{inspect_source, open_las_reader},
        spatial::spool_source,
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
        let options = ConvertOptions::new(BATCH, 0.01).unwrap();
        let reader = open_las_reader(path).unwrap();
        let info = inspect_source(&reader).unwrap();
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
            let mut progress = |_: crate::ConversionProgress| true;
            match range {
                None => spool_source(
                    &mut EndsAt(LasPointSource::full(reader), end),
                    &info,
                    options,
                    &store,
                    id,
                    &mut progress,
                    false,
                )?,
                Some(range) => spool_source(
                    &mut EndsAt(LasPointSource::records(reader, range)?, end),
                    &info,
                    options,
                    &store,
                    id,
                    &mut progress,
                    false,
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
