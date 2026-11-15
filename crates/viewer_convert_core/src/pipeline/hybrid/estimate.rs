//! Pre-run estimate of spool peaks and spill zones, from the header alone (record
//! `HYBRID_POLICY_2026-10-30`, section "Estimacion previa a la conversion"). Integer
//! arithmetic only: the conversion core forbids floats and libm.

use super::store::HybridConfig;
use crate::error::ConvertError;
use crate::options::ConvertOptions;
use crate::pipeline::point_source::{ByteSource, LasFactory, PointSource, SourceFactory};
use crate::source::{SourceInfo, inspect_header};
use crate::spatial::SpoolLayout;

/// Upper bounds for one run (single worker, depth-first order, trimmed segments).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpoolEstimate {
    /// `S`: every source point as one spool record.
    pub source_spool_bytes: u64,
    /// Peak of live spool payload.
    pub live_peak_bytes: u64,
    /// Spool count (octree nodes), each wasting about half a page when spilled.
    pub nodes: u64,
    /// Peak spill-tier bytes; 0 when the live peak fits the RAM budget.
    pub spilled_peak_bytes: u64,
    /// Zones to open for the spill tier (at least 1).
    pub zones: u32,
    /// Estimated size of the converted output.
    pub output_bytes: u64,
    /// RAM needed when no spill tier exists (live peak plus margin plus output).
    pub ram_only_bytes: u64,
}

/// Reads the LAS/LAZ header of `input` without decoding points.
pub fn inspect_source<B: ByteSource>(input: &LasFactory<B>) -> Result<SourceInfo, ConvertError> {
    let full = input
        .split(1, 1)?
        .into_iter()
        .next()
        .expect("split returns at least one range");
    let reader = input.open(full)?;
    inspect_header(reader.header())
}

pub fn estimate_spools(
    source: &SourceInfo,
    options: ConvertOptions,
    config: HybridConfig,
) -> Result<SpoolEstimate, ConvertError> {
    let options = options.validate()?;
    let points = u128::from(source.point_count);
    let record = SpoolLayout::source(source).record_bytes() as u128;
    let divisions = u128::from(options.spatial_divisions()).max(1);
    let block = u128::from(options.points_per_block()).max(1);
    let d2 = divisions * divisions;

    let s = points * record;
    let live = (2 * s).min(s * 11 * (2 * d2 + 5) / (20 * d2));
    let nodes = (12 * divisions * points).div_ceil(5 * block);
    let page = u128::from(config.page_bytes);
    let spilled = if live * 103 > u128::from(config.ram_budget_bytes) * 100 {
        live + nodes * page / 2
    } else {
        0
    };
    let zones = (2 * spilled).div_ceil(u128::from(config.zone_bytes).max(1)) + 1;
    let output = 3 * s / 5;
    let ram_only = live * 103 / 100 + output;

    let narrow = |value: u128| u64::try_from(value).map_err(|_| ConvertError::SizeOverflow);
    Ok(SpoolEstimate {
        source_spool_bytes: narrow(s)?,
        live_peak_bytes: narrow(live)?,
        nodes: narrow(nodes)?,
        spilled_peak_bytes: narrow(spilled)?,
        zones: u32::try_from(zones).map_err(|_| ConvertError::SizeOverflow)?,
        output_bytes: narrow(output)?,
        ram_only_bytes: narrow(ram_only)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{CoordinateFrame, SpatialBounds};

    const GIB: f64 = (1_u64 << 30) as f64;

    // Point counts: source points from REAL_DATASET_BENCH / SENSOFOREST_V6_BENCH (the record's
    // inputs); record bytes 41 (format 3) and 33 (format 2) as in the record.
    const BELVEDERE: u64 = 81_682_329;
    const MAJADAS: u64 = 415_465_882;

    fn info(points: u64, format: u8) -> SourceInfo {
        SourceInfo {
            bounds: SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
            frame: CoordinateFrame::new([0.0; 3]).unwrap(),
            point_count: points,
            record_length: 0,
            point_format: las::point::Format::new(format).unwrap(),
            transforms: las::Vector::default(),
            has_crs: false,
            crs: viewer_format::CrsRecord::local(),
            scale_offset: None,
            global: None,
            extra_byte_vlrs: Vec::new(),
            las_extra_bytes: None,
        }
    }

    fn estimate(points: u64, format: u8, divisions: u8, budget: u64) -> SpoolEstimate {
        let options = ConvertOptions::default()
            .with_format_version(viewer_format::FormatVersion::V5)
            .unwrap()
            .with_spatial_divisions(divisions)
            .unwrap();
        estimate_spools(&info(points, format), options, HybridConfig::web(budget)).unwrap()
    }

    #[test]
    fn matches_the_record_table() {
        let budget = 2 << 30;
        let cases = [
            (BELVEDERE, 3, 2, 1496, 13, 5.58, 7.61),
            (BELVEDERE, 3, 8, 5983, 10, 3.56, 5.54),
            (MAJADAS, 2, 2, 7608, 49, 22.82, 31.2),
        ];
        for (points, format, divisions, nodes, zones, live, ram_only) in cases {
            let e = estimate(points, format, divisions, budget);
            assert_eq!(e.nodes, nodes);
            assert_eq!(e.zones, zones);
            assert!((e.live_peak_bytes as f64 / GIB - live).abs() < 0.01);
            assert!((e.ram_only_bytes as f64 / GIB - ram_only).abs() < 0.05);
        }
    }

    #[test]
    fn small_live_peak_needs_one_zone_and_no_spill() {
        let e = estimate(1_000, 3, 2, 2 << 30);
        assert_eq!(e.spilled_peak_bytes, 0);
        assert_eq!(e.zones, 1);
    }

    #[test]
    fn estimates_grow_with_points_and_shrink_with_budget() {
        let small = estimate(BELVEDERE, 3, 2, 2 << 30);
        let large = estimate(2 * BELVEDERE, 3, 2, 2 << 30);
        assert!(large.live_peak_bytes > small.live_peak_bytes);
        assert!(large.nodes >= small.nodes);
        assert!(large.zones >= small.zones);
        assert!(large.ram_only_bytes > small.ram_only_bytes);
        let roomy = estimate(BELVEDERE, 3, 2, 16 << 30);
        assert_eq!(roomy.spilled_peak_bytes, 0);
        assert!(roomy.zones <= small.zones);
    }

    #[test]
    fn overflow_is_reported() {
        let result = estimate_spools(
            &info(u64::MAX, 3),
            ConvertOptions::default(),
            HybridConfig::web(0),
        );
        assert!(matches!(result, Err(ConvertError::SizeOverflow)));
    }
}
