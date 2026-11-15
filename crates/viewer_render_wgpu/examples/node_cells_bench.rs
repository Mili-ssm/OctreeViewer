//! Walks every node of a real dataset through `PointNodeCellSource` and reports the figures the
//! R3 stop criteria depend on. Not part of `scripts/check.sh`.
//!
//! Usage: node_cells_bench INPUT.oct [MAX_ROWS_PRINTED=10]
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    time::Instant,
};

use viewer_core::{DatasetSource, Point};
use viewer_format::{FormatReader, OctreeReader, PagedReader};
use viewer_render_wgpu::{
    CHAIN_MIN_GROWTH, CellGrid, NodeCell, NodeCellSource, PointNodeCellSource, chain_keep_mask,
    representative_depth, representative_depth_for_spacing,
};

/// Planned GPU instance size (ADR 0027): 512 instances per 8 KiB page.
const INSTANCE_BYTES: usize = 16;
/// Growth values compared against the unpruned chain (`1.0`), besides `CHAIN_MIN_GROWTH`.
const CANDIDATE_GROWTH: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

#[derive(Default, Clone, Copy)]
struct GrowthStats {
    over_4x: usize,
    worst: f64,
    cell_bytes: usize,
    levels: usize,
}

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
    let mut attrs_path = args[0].clone();
    attrs_path.push("attrs");
    if !reader.has_attributes() {
        reader.attach_attributes(File::open(attrs_path)?)?;
    }
    // The paged hierarchy records the authoritative bit even for an unloaded anchor.
    let mut leaf_flags = BTreeMap::new();
    if reader.format_version().uses_pages() {
        let mut paged = PagedReader::open(File::open(&args[0])?)?;
        let mut pages = vec![paged.root_page().clone()];
        let mut cursor = 0;
        while cursor < pages.len() {
            let page = &pages[cursor];
            for node in page.nodes() {
                if leaf_flags.insert(node.id(), !node.has_children()).is_some() {
                    return Err(format!("duplicate paged node ID {}", node.id().get()).into());
                }
            }
            let children: Vec<_> = page.child_pages().collect();
            for (anchor, pointer) in children {
                pages.push(paged.read_child_page(anchor, pointer)?);
            }
            cursor += 1;
        }
    }
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
    if reader.format_version().uses_pages() {
        let queried: BTreeSet<_> = nodes.iter().map(|node| node.id()).collect();
        let paged: BTreeSet<_> = leaf_flags.keys().copied().collect();
        if queried.len() != nodes.len() || queried != paged {
            return Err(format!("node traversal mismatch: queried {} entries / {} unique, paged {} unique; missing {}, unexpected {}", nodes.len(), queried.len(), paged.len(), paged.difference(&queried).count(), queried.difference(&paged).count()).into());
        }
        eprintln!(
            "verified all {} hierarchy node IDs against paged traversal",
            paged.len()
        );
    }
    let mut non_cubic = Vec::new();
    let mut unexpected_depth = Vec::new();
    let mut multi_block = 0_usize;
    let mut depth_by_level = BTreeMap::<(u32, u8), usize>::new();
    let mut chain_histogram = BTreeMap::<usize, usize>::new();
    let (mut leaves, mut leaves_at_cap, mut duplicate_only, mut over_4x) =
        (0_usize, 0_usize, 0_usize, 0_usize);
    let (mut leaf_seconds, mut leaf_points, mut leaf_cell_bytes, mut leaf_point_bytes) =
        (0.0_f64, 0_u64, 0_usize, 0_usize);
    let mut worst_ratio = 0.0_f64;
    let mut worst_leaf = String::new();
    let (mut internal_seconds, mut internal_nodes) = (0.0_f64, 0_usize);
    let mut cells_total = 0_usize;
    let mut cells_by_depth = BTreeMap::<u8, usize>::new();
    let mut regions_by_depth = BTreeMap::<u8, usize>::new();
    let mut peak_nominal_key_bytes = 0_usize;
    let mut growth_stats = [GrowthStats::default(); CANDIDATE_GROWTH.len()];

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
        multi_block += usize::from(node.blocks().len() > 1);
        let local = viewer_core::SpatialBounds::new(
            std::array::from_fn(|axis| bounds.min()[axis] - origin[axis]),
            std::array::from_fn(|axis| bounds.max()[axis] - origin[axis]),
        )?;
        let first = node.spacing().map_or_else(
            || representative_depth(&grid, local),
            |spacing| representative_depth_for_spacing(&grid, spacing),
        );
        *depth_by_level.entry((node.level(), first)).or_default() += 1;
        let blocks: Vec<_> = node
            .blocks()
            .iter()
            .map(|id| reader.read_block(*id))
            .collect::<Result<_, _>>()?;
        let point_count: usize = blocks.iter().map(|block| block.points().len()).sum();
        let finite_count = blocks
            .iter()
            .flat_map(|block| block.points().points())
            .filter(|p| p.position.iter().all(|v| v.is_finite()))
            .count();
        let is_leaf = if leaf_flags.is_empty() {
            hierarchy.is_leaf(node.id())
        } else {
            *leaf_flags
                .get(&node.id())
                .ok_or("node missing authoritative has_children flag")?
        };
        let started = Instant::now();
        // Growth 1.0 keeps every level: the chain before pruning.
        let cells = PointNodeCellSource { min_growth: 1.0 }
            .node_cells(&grid, node.id(), local, node.spacing(), is_leaf, &blocks)
            .map_err(|error| {
                let mut bins = std::collections::BTreeSet::new();
                let mut positions = std::collections::BTreeSet::new();
                let mut finite = 0;
                for block in &blocks {
                    for point in block.points().points() {
                        if point.position.iter().any(|v| !v.is_finite()) { continue; }
                        finite += 1;
                        positions.insert(point.position.map(|value| if value == 0.0 { 0 } else { value.to_bits() }));
                        bins.insert(std::array::from_fn::<_, 3, _>(|axis| {
                            (((f64::from(point.position[axis]) - local.min()[axis]) / side) * (1_u64 << 21) as f64).floor() as u64
                        }));
                    }
                }
                format!(
                    "node {} level {} leaf {is_leaf} spacing {:?} bounds {:?} root_side {} first_depth {first} finite {finite} unique_positions {} occupied_21bit_cells {}: {error}",
                    node.id().get(),
                    node.level(),
                    node.spacing(),
                    local, grid.root_side, positions.len(), bins.len()
                )
            })?;
        let seconds = started.elapsed().as_secs_f64();
        // The finest built level has the widest coordinate range and is always retained.
        if let Some(level) = cells.levels.last() {
            let count = 1_u64 << level.depth;
            let cell_side = grid.root_side / count as f64;
            let single_region = (0..3).all(|axis| {
                let last = (count - 1) as f64;
                let lo = ((local.min()[axis] - grid.root_min[axis]) / cell_side)
                    .round()
                    .clamp(0.0, last);
                let hi = (((local.max()[axis] - grid.root_min[axis]) / cell_side).round() - 1.0)
                    .clamp(lo, last);
                (hi as u64) - (lo as u64) < (1 << 21)
            });
            let record_size = if single_region {
                std::mem::size_of::<(u64, usize, usize, [u64; 3])>()
            } else {
                std::mem::size_of::<((u128, u64), usize, usize, [u64; 3])>()
            };
            peak_nominal_key_bytes = peak_nominal_key_bytes.max(finite_count * record_size);
        }
        let cell_count: usize = cells.levels.iter().map(|level| level.cells.len()).sum();
        cells_total += cell_count;
        for level in &cells.levels {
            *cells_by_depth.entry(level.depth).or_default() += level.cells.len();
            let count = (1_u64 << level.depth) as f64;
            let cell_side = grid.root_side / count;
            let node_origin: [u64; 3] = std::array::from_fn(|axis| {
                ((local.min()[axis] - grid.root_min[axis]) / cell_side)
                    .round()
                    .clamp(0.0, (count - 1.0).max(0.0)) as u64
            });
            let regions: BTreeSet<_> = level
                .cells
                .iter()
                .map(|cell| {
                    std::array::from_fn::<_, 3, _>(|axis| {
                        (cell.coord[axis] - node_origin[axis]) >> 21
                    })
                })
                .collect();
            *regions_by_depth.entry(level.depth).or_default() += regions.len();
        }
        if is_leaf {
            leaves += 1;
            *chain_histogram.entry(cells.levels.len()).or_default() += 1;
            if let Some(last) = cells.levels.last() {
                let positions = blocks
                    .iter()
                    .flat_map(|block| block.points().points().iter().map(|point| point.position));
                match classify_cap(positions, last.cells.len()) {
                    CapStatus::DistinctCollision => leaves_at_cap += 1,
                    CapStatus::DuplicatesOnly => duplicate_only += 1,
                    CapStatus::InvariantError => {
                        return Err(format!(
                            "node {} has more occupied cells than unique finite positions",
                            node.id().get()
                        )
                        .into());
                    }
                    CapStatus::Separated => {}
                }
            }
            let cell_bytes = cell_count * INSTANCE_BYTES;
            let point_bytes = point_count * std::mem::size_of::<Point>();
            if cell_bytes > 4 * point_bytes {
                over_4x += 1;
            }
            if point_bytes > 0 && cell_bytes as f64 / point_bytes as f64 > worst_ratio {
                worst_ratio = cell_bytes as f64 / point_bytes as f64;
                let per_level: Vec<_> = cells
                    .levels
                    .iter()
                    .map(|level| (level.depth, level.cells.len()))
                    .collect();
                worst_leaf = format!(
                    "node {} level {} spacing {:?}: {point_count} points, (depth, cells) {per_level:?}",
                    node.id().get(),
                    node.level(),
                    node.spacing()
                );
            }
            let counts: Vec<usize> = cells.levels.iter().map(|level| level.cells.len()).collect();
            for (stats, growth) in growth_stats.iter_mut().zip(CANDIDATE_GROWTH) {
                let mask = chain_keep_mask(&counts, growth);
                let kept = counts.iter().zip(&mask).filter(|(_, keep)| **keep);
                let bytes = kept.clone().map(|(count, _)| count).sum::<usize>() * INSTANCE_BYTES;
                stats.over_4x += usize::from(bytes > 4 * point_bytes);
                if point_bytes > 0 {
                    stats.worst = stats.worst.max(bytes as f64 / point_bytes as f64);
                }
                stats.cell_bytes += bytes;
                stats.levels += kept.count();
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
        "nodes: {} (leaves {leaves}, internal {internal_nodes}), nodes with several blocks: {multi_block}",
        nodes.len()
    );
    println!("size_of NodeCell: {} B", std::mem::size_of::<NodeCell>());
    println!(
        "Byte ratios compare projected 16-byte GPU instance payload with Point struct payload; they exclude allocation capacity, attributes, page metadata and other overhead."
    );
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
            "leaves with unresolved distinct-position collisions at terminal depth (experimental): {leaves_at_cap} ({:.3} %)",
            100.0 * leaves_at_cap as f64 / leaves as f64
        );
        println!(
            "leaves with only duplicate positions at terminal depth (experimental): {duplicate_only}"
        );
        println!(
            "leaves with unpruned chain > 4x point bytes: {over_4x}; worst ratio {worst_ratio:.2}x; \
             total unpruned chain bytes {leaf_cell_bytes} vs point bytes {leaf_point_bytes} ({:.2}x)",
            leaf_cell_bytes as f64 / leaf_point_bytes.max(1) as f64
        );
        println!(
            "unpruned leaf build (all levels): {:.3} ms/leaf, {:.1} ns/point, {:.1} unpruned chain B/leaf, {:.1} point B/leaf",
            1000.0 * leaf_seconds / leaves as f64,
            1.0e9 * leaf_seconds / leaf_points.max(1) as f64,
            leaf_cell_bytes as f64 / leaves as f64,
            leaf_point_bytes as f64 / leaves as f64,
        );
    }
    println!("worst unpruned leaf: {worst_leaf}");
    println!("leaf chain pruning (default growth {CHAIN_MIN_GROWTH}; 1.0 = no pruning):");
    for (stats, growth) in growth_stats.iter().zip(CANDIDATE_GROWTH) {
        println!(
            "  growth {growth}: >4x {}, worst {:.2}x, total {:.2}x, {:.2} levels/leaf",
            stats.over_4x,
            stats.worst,
            stats.cell_bytes as f64 / leaf_point_bytes.max(1) as f64,
            stats.levels as f64 / leaves.max(1) as f64,
        );
    }
    if internal_nodes > 0 {
        println!(
            "internal build: {:.3} ms/node",
            1000.0 * internal_seconds / internal_nodes as f64
        );
    }
    println!("cells by depth: {cells_by_depth:?}");
    println!("occupied regions summed per node by depth: {regions_by_depth:?}");
    println!(
        "peak nominal temporary key payload: {peak_nominal_key_bytes} bytes (finite points times narrow or wide record size at finest level; excludes allocation capacity and other memory; not RSS)"
    );
    println!(
        "total cells: {cells_total}; CPU NodeCell payload bytes: {}; projected GPU instance payload bytes: {}",
        cells_total * std::mem::size_of::<NodeCell>(),
        cells_total * INSTANCE_BYTES
    );
    if leaves_at_cap > 0 {
        return Err("leaf chains have unresolved distinct-position collisions at depth cap".into());
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
enum CapStatus {
    DistinctCollision,
    DuplicatesOnly,
    InvariantError,
    Separated,
}

fn classify_cap(positions: impl Iterator<Item = [f32; 3]>, cells: usize) -> CapStatus {
    let mut unique = BTreeSet::new();
    let mut finite = 0;
    for position in positions {
        if position.iter().any(|value| !value.is_finite()) {
            continue;
        }
        finite += 1;
        unique.insert(position.map(|value| if value == 0.0 { 0 } else { value.to_bits() }));
    }
    match cells.cmp(&unique.len()) {
        std::cmp::Ordering::Less => CapStatus::DistinctCollision,
        std::cmp::Ordering::Greater => CapStatus::InvariantError,
        std::cmp::Ordering::Equal if unique.len() < finite => CapStatus::DuplicatesOnly,
        std::cmp::Ordering::Equal => CapStatus::Separated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_finite_positions_across_blocks_at_cap() {
        let blocks = [
            vec![[0.0, -0.0, 1.0], [1.0, 2.0, 3.0], [f32::NAN, 0.0, 0.0]],
            vec![[-0.0, 0.0, 1.0], [f32::INFINITY, 0.0, 0.0]],
        ];
        let positions = || blocks.iter().flat_map(|block| block.iter().copied());
        assert_eq!(classify_cap(positions(), 1), CapStatus::DistinctCollision);
        assert_eq!(classify_cap(positions(), 2), CapStatus::DuplicatesOnly);
        assert_eq!(classify_cap(positions(), 3), CapStatus::InvariantError);
        assert_eq!(
            classify_cap([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]].into_iter(), 2),
            CapStatus::Separated
        );
    }
}
