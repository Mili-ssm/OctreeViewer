//! Density spike for render R4 (plan `docs/plans/render-r4-piramide-densa.md`, task 1).
//!
//! Usage: `density_spike INPUT OUTPUT_DIR --leaf-points N --sample-grid G`
//!
//! Converts INPUT (anything the converter accepts) to `OUTPUT_DIR/spike.oct` as `v7.3-pre` with
//! `points_per_block = N` (the leaf limit) and an internal `sample_grid = G`, then reads the
//! output back and prints one JSON object: wall time of the conversion, peak RSS (`VmHWM`, read
//! right after the conversion), output bytes (`.oct` + `.octattrs`), node/leaf counts, max level,
//! points per leaf (p50/p95/max) and the leaf/parent spacing jump (p50/p95/max).
//!
//! Spacing jump, per leaf that has a parent:
//! - parent spacing = `parent_side / G`, the cell size of the parent's G^3 representatives;
//! - leaf spacing = `leaf_side / 2^d`, where the leaf cube is subdivided dyadically and `d` is the
//!   first level (0 = the leaf cube itself) at which the number of occupied cells grows by less
//!   than x1.5 against the previous level (capped at 16);
//! - jump = `log2(parent_spacing / leaf_spacing) - 1`, in levels (0 = perfect pyramid, where each
//!   level halves the spacing).
//!
//! Cells are computed from the decoded `f32` positions (frame-relative) against the node bounds, so very small
//! cells (side / 2^d below f32 resolution) are approximate. Measurement tool: `log2` is fine here.
#![allow(
    clippy::disallowed_methods,
    reason = "measurement tool, not a format path (ADR 0016)"
)]

use std::{collections::HashSet, env, fs, fs::File, path::PathBuf, time::Instant};

use serde_json::json;
use viewer_convert::{ConvertOptions, convert_file};
use viewer_core::NodeId;
use viewer_format::{FormatReader, FormatVersion};

const MAX_DEPTH: u32 = 16;
const GROWTH_THRESHOLD: f64 = 1.5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut leaf_points: Option<u64> = None;
    let mut sample_grid: Option<usize> = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--leaf-points" => leaf_points = Some(args.next().ok_or("missing N")?.parse()?),
            "--sample-grid" => sample_grid = Some(args.next().ok_or("missing G")?.parse()?),
            _ => positional.push(arg),
        }
    }
    let [input, output_dir] = positional.as_slice() else {
        return Err("usage: density_spike INPUT OUTPUT_DIR --leaf-points N --sample-grid G".into());
    };
    let leaf_points = leaf_points.ok_or("--leaf-points is required")?;
    let sample_grid = sample_grid.ok_or("--sample-grid is required")?;

    let output_dir = PathBuf::from(output_dir);
    fs::create_dir_all(&output_dir)?;
    let output = output_dir.join("spike.oct");
    // The converter refuses to overwrite; a previous spike run is scratch data.
    for stale in [output.clone(), output.with_extension("octattrs")] {
        if stale.exists() {
            fs::remove_file(stale)?;
        }
    }
    let options = ConvertOptions::new(
        leaf_points,
        ConvertOptions::default().max_quantization_error(),
    )?
    .with_format_version(FormatVersion::V7_PRE3)?
    .with_sample_grid(sample_grid)?;

    let started = Instant::now();
    convert_file(input.as_ref(), &output, options)?;
    let wall_seconds = started.elapsed().as_secs_f64();
    let peak_rss_bytes = peak_rss_bytes();

    let attrs = output.with_extension("octattrs");
    let output_bytes = fs::metadata(&output)?.len() + fs::metadata(&attrs).map_or(0, |m| m.len());

    let mut reader = FormatReader::new(File::open(&output)?)?;
    let hierarchy = reader.format_metadata().dataset().hierarchy().clone();
    let origin = reader
        .format_metadata()
        .dataset()
        .coordinate_frame()
        .origin();
    let mut stack: Vec<NodeId> = vec![hierarchy.root_id()];
    let (mut node_count, mut leaf_count) = (0_u64, 0_u64);
    let mut leaf_sizes = Vec::new();
    let mut jumps = Vec::new();
    while let Some(id) = stack.pop() {
        let node = hierarchy.node(id).ok_or("dangling node id")?;
        node_count += 1;
        stack.extend_from_slice(hierarchy.child_ids(id));
        if !hierarchy.is_leaf(id) {
            continue;
        }
        leaf_count += 1;
        leaf_sizes.push(node.point_count() as f64);
        let Some(parent) = node.parent().and_then(|p| hierarchy.node(p)) else {
            continue;
        };
        // Node bounds are world coordinates; decoded positions are relative to the frame origin.
        let min: [f64; 3] = std::array::from_fn(|axis| node.bounds().min()[axis] - origin[axis]);
        let side = side(node.bounds());
        let mut positions = Vec::with_capacity(node.point_count() as usize);
        for block in node.blocks() {
            for point in reader.read_positions(*block)?.points() {
                positions.push(point.position.map(f64::from));
            }
        }
        let depth = leaf_depth(&positions, min, side);
        let leaf_spacing = side / f64::from(1_u32 << depth);
        let parent_spacing = self::side(parent.bounds()) / sample_grid as f64;
        jumps.push((parent_spacing / leaf_spacing).log2() - 1.0);
    }
    let max_level = hierarchy.stats().max_level;

    leaf_sizes.sort_by(f64::total_cmp);
    jumps.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "input": input,
            "leaf_points": leaf_points,
            "sample_grid": sample_grid,
            "wall_seconds": wall_seconds,
            "peak_rss_bytes": peak_rss_bytes,
            "output_bytes": output_bytes,
            "node_count": node_count,
            "leaf_count": leaf_count,
            "max_level": max_level,
            "points_per_leaf": summary(&leaf_sizes),
            "spacing_jump_levels": summary(&jumps),
        }))?
    );
    Ok(())
}

fn side(bounds: viewer_core::SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

/// First level `d` in `1..=MAX_DEPTH` whose occupied-cell count grows by less than x1.5 against
/// level `d - 1`; `MAX_DEPTH` if it never does.
fn leaf_depth(positions: &[[f64; 3]], min: [f64; 3], side: f64) -> u32 {
    let mut previous = 1_usize;
    for depth in 1..=MAX_DEPTH {
        let cells = f64::from(1_u32 << depth);
        let occupied = positions
            .iter()
            .map(|p| {
                let cell = |axis: usize| {
                    (((p[axis] - min[axis]) / side * cells).floor().max(0.0) as u32)
                        .min((1_u32 << depth) - 1)
                };
                (cell(0), cell(1), cell(2))
            })
            .collect::<HashSet<_>>()
            .len();
        if (occupied as f64) < GROWTH_THRESHOLD * previous as f64 {
            return depth;
        }
        previous = occupied;
    }
    MAX_DEPTH
}

/// Nearest-rank percentiles of a sorted sample.
fn summary(sorted: &[f64]) -> serde_json::Value {
    let rank = |q: f64| {
        sorted
            .get(((q * sorted.len() as f64).ceil() as usize).saturating_sub(1))
            .copied()
    };
    json!({ "p50": rank(0.5), "p95": rank(0.95), "max": sorted.last() })
}

fn peak_rss_bytes() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    Some(line.split_whitespace().nth(1)?.parse::<u64>().ok()? * 1024)
}
