//! Walks every node of a real dataset through `PointNodeCellSource` and reports the figures the
//! R3 stop criteria depend on. Not part of `scripts/check.sh`.
//!
//! Usage: node_cells_bench INPUT.oct [MAX_ROWS_PRINTED=10]
use std::{collections::BTreeMap, fs::File, time::Instant};

use viewer_core::{Point, PointBlock};
use viewer_format::FormatReader;
use viewer_render_wgpu::{
    CHAIN_SPAN, CellGrid, MAX_CELL_DEPTH, NodeCell, NodeCellSource, PointNodeCellSource,
    representative_depth, representative_depth_for_spacing,
};

/// Planned GPU instance size (ADR 0027): 512 instances per 8 KiB page.
const INSTANCE_BYTES: usize = 16;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        return Err("usage: node_cells_bench INPUT.oct [MAX_ROWS_PRINTED=10]".into());
    }
    let max_rows = args
        .get(1)
        .map(|value| value.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(10);
    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let viewer_dataset = reader.format_metadata().viewer_dataset()?;
    let metadata = &*viewer_dataset;
    let hierarchy = metadata.hierarchy().clone();
    let origin = metadata.coordinate_frame().origin();
    let root = hierarchy.stats().root_bounds;
    let root_side = (0..3)
        .map(|axis| root.max()[axis] - root.min()[axis])
        .fold(0.0, f64::max);
    let grid = CellGrid {
        root_min: std::array::from_fn(|axis| root.min()[axis] - origin[axis]),
        root_side,
    };

    let nodes: Vec<_> = hierarchy.query(root).cloned().collect();
    let mut non_cubic = Vec::new();
    let mut unexpected_depth = Vec::new();
    let mut multi_block = 0_usize;
    let mut depth_by_level = BTreeMap::<(u32, u8), usize>::new();
    let mut chain_histogram = BTreeMap::<usize, usize>::new();
    let (mut leaves, mut leaves_at_cap, mut over_4x) = (0_usize, 0_usize, 0_usize);
    let (mut leaf_seconds, mut leaf_points, mut leaf_cell_bytes, mut leaf_point_bytes) =
        (0.0_f64, 0_u64, 0_usize, 0_usize);
    let mut worst_ratio = 0.0_f64;
    let (mut internal_seconds, mut internal_nodes) = (0.0_f64, 0_usize);
    let mut cells_total = 0_usize;

    for node in &nodes {
        let bounds = node.bounds();
        let sides: [f64; 3] = std::array::from_fn(|axis| bounds.max()[axis] - bounds.min()[axis]);
        let side = sides.iter().copied().fold(0.0, f64::max);
        let shortest = sides.iter().copied().fold(f64::INFINITY, f64::min);
        if shortest < side * (1.0 - 1.0e-6) {
            non_cubic.push((node.id().get(), sides));
        }
        // 8 cells per axis means the root/side ratio is a power of two once divided by 8.
        let exact = (grid.root_side / node.representative_spacing()).log2();
        if (exact - exact.round()).abs() > 1.0e-6 {
            unexpected_depth.push((node.id().get(), node.level(), exact));
        }
        if node.blocks().len() != 1 {
            multi_block += usize::from(node.blocks().len() > 1);
            continue;
        }
        let local = viewer_core::SpatialBounds::new(
            std::array::from_fn(|axis| bounds.min()[axis] - origin[axis]),
            std::array::from_fn(|axis| bounds.max()[axis] - origin[axis]),
        )?;
        let first = node.spacing().map_or_else(
            || representative_depth(&grid, local),
            |spacing| representative_depth_for_spacing(&grid, spacing),
        );
        *depth_by_level.entry((node.level(), first)).or_default() += 1;
        let points = reader.read_positions(node.blocks()[0])?;
        let point_count = points.len();
        let block = PointBlock::new(node.blocks()[0], points, Vec::new());
        let is_leaf = hierarchy.is_leaf(node.id());
        let started = Instant::now();
        let cells = PointNodeCellSource.node_cells(
            &grid,
            node.id(),
            local,
            node.spacing(),
            is_leaf,
            &block,
        )?;
        let seconds = started.elapsed().as_secs_f64();
        let cell_count: usize = cells.levels.iter().map(|level| level.cells.len()).sum();
        cells_total += cell_count;
        if is_leaf {
            leaves += 1;
            *chain_histogram.entry(cells.levels.len()).or_default() += 1;
            let cap = first.saturating_add(CHAIN_SPAN).min(MAX_CELL_DEPTH);
            // The chain only reaches its last allowed depth while cells were still growing.
            if cells.levels.last().is_some_and(|level| level.depth == cap) {
                leaves_at_cap += 1;
            }
            let cell_bytes = cell_count * INSTANCE_BYTES;
            let point_bytes = point_count * std::mem::size_of::<Point>();
            if cell_bytes > 4 * point_bytes {
                over_4x += 1;
            }
            if point_bytes > 0 {
                worst_ratio = worst_ratio.max(cell_bytes as f64 / point_bytes as f64);
            }
            leaf_seconds += seconds;
            leaf_points += point_count as u64;
            leaf_cell_bytes += cell_bytes;
            leaf_point_bytes += point_bytes;
        } else {
            internal_seconds += seconds;
            internal_nodes += 1;
        }
    }

    println!("file: {}", args[0].to_string_lossy());
    println!(
        "nodes: {} (leaves {leaves}, internal {internal_nodes}), nodes with several blocks (skipped): {multi_block}",
        nodes.len()
    );
    println!("size_of NodeCell: {} B", std::mem::size_of::<NodeCell>());
    println!("non-cubic nodes: {}", non_cubic.len());
    for (id, sides) in non_cubic.iter().take(max_rows) {
        println!("  node {id}: sides {sides:?}");
    }
    println!(
        "nodes whose 8-cells-per-axis depth is not an integer: {}",
        unexpected_depth.len()
    );
    for (id, level, exact) in unexpected_depth.iter().take(max_rows) {
        println!("  node {id} level {level}: log2 ratio {exact}");
    }
    println!("representative depth by (level, depth): count");
    for ((level, depth), count) in &depth_by_level {
        println!("  level {level} depth {depth}: {count}");
    }
    println!("leaf chain length histogram (levels: leaves)");
    for (length, count) in &chain_histogram {
        println!("  {length}: {count}");
    }
    if leaves > 0 {
        println!(
            "leaves at chain cap: {leaves_at_cap} ({:.3} %)",
            100.0 * leaves_at_cap as f64 / leaves as f64
        );
        println!(
            "leaves with chain > 4x point bytes: {over_4x}; worst ratio {worst_ratio:.2}x; \
             total chain bytes {leaf_cell_bytes} vs point bytes {leaf_point_bytes} ({:.2}x)",
            leaf_cell_bytes as f64 / leaf_point_bytes.max(1) as f64
        );
        println!(
            "leaf build: {:.3} ms/leaf, {:.1} ns/point, {:.1} chain B/leaf, {:.1} point B/leaf",
            1000.0 * leaf_seconds / leaves as f64,
            1.0e9 * leaf_seconds / leaf_points.max(1) as f64,
            leaf_cell_bytes as f64 / leaves as f64,
            leaf_point_bytes as f64 / leaves as f64,
        );
    }
    if internal_nodes > 0 {
        println!(
            "internal build: {:.3} ms/node",
            1000.0 * internal_seconds / internal_nodes as f64
        );
    }
    println!("total cells: {cells_total}");
    Ok(())
}
