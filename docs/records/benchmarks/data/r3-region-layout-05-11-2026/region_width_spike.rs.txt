//! Throwaway R3 spike: for every stored level of a real v7.5 dataset, models the GPU page layout
//! per region width `W` (bits per axis) and flag placement, and prints machine-readable TSV.
//! Not part of `scripts/check.sh`. See `docs/plans/r3-anchura-de-region.md`, task 1.
//!
//! Usage: region_width_spike INPUT.oct [MAX_NODES=0]   (0 = all nodes)
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::File,
    hint::black_box,
    time::{Duration, Instant},
};

use viewer_core::{DatasetSource, SpatialBounds};
use viewer_format::{FormatReader, OctreeReader, PagedReader};
use viewer_render_wgpu::{
    CellGrid, NodeCell, NodeCellSource, PointNodeCellSource, node_cell_origin,
};

const WIDTHS: [u32; 7] = [8, 10, 12, 16, 21, 24, 32];
const PAGE_BYTES: usize = 8192;
/// Nominal directory entry per page (plan assumption).
const PAGE_ENTRY_BYTES: usize = 32;
/// rgba8 + u16 intensity + i16 classification.
const FIXED_PAYLOAD_BYTES: usize = 8;
const REPS: usize = 3;
const HIST_BUCKETS: usize = 16;
const COMBOS: usize = WIDTHS.len() * 2;
const MODELS: [&str; 2] = ["a_aligned", "b_greedy"];
const GROUPS: [&str; 5] = [
    "internal",
    "leaf_non_finest",
    "leaf_finest",
    "proxy_far",
    "proxy_near",
];
const BITS_ALARM: u32 = 24;

#[derive(Clone, Copy)]
struct Combo {
    width: u32,
    flags_in_instance: bool,
}

impl Combo {
    fn all() -> [Combo; COMBOS] {
        std::array::from_fn(|index| Combo {
            width: WIDTHS[index / 2],
            flags_in_instance: index % 2 == 1,
        })
    }
    fn flag_name(self) -> &'static str {
        if self.flags_in_instance {
            "instance"
        } else {
            "page"
        }
    }
    fn instance_bytes(self) -> usize {
        instance_bytes(self.width, self.flags_in_instance)
    }
    fn capacity(self) -> usize {
        PAGE_BYTES / self.instance_bytes()
    }
}

fn position_words(width: u32, flags_in_instance: bool) -> usize {
    (3 * width as usize + if flags_in_instance { 3 } else { 0 }).div_ceil(32)
}

fn instance_bytes(width: u32, flags_in_instance: bool) -> usize {
    4 * position_words(width, flags_in_instance) + FIXED_PAYLOAD_BYTES
}

#[derive(Debug, Clone, PartialEq)]
struct Page {
    start: usize,
    end: usize,
    origin: [u64; 3],
}

/// Model (a): aligned blocks of 2^W relative to the node origin, split when over capacity.
fn layout_aligned(rels: &[[u64; 3]], width: u32, capacity: usize) -> Result<Vec<Page>, String> {
    let mut pages = Vec::new();
    let mut seen = HashSet::new();
    let mut start = 0;
    let mut key = [u64::MAX; 3];
    for (index, rel) in rels.iter().enumerate() {
        let next = rel.map(|value| value >> width);
        if index > start && (next != key || index - start == capacity) {
            pages.push(Page {
                start,
                end: index,
                origin: key.map(|k| k << width),
            });
            start = index;
        }
        if next != key || index == 0 {
            if !seen.insert(next) {
                return Err(format!("aligned block {next:?} is not contiguous"));
            }
            key = next;
        }
    }
    if start < rels.len() {
        pages.push(Page {
            start,
            end: rels.len(),
            origin: key.map(|k| k << width),
        });
    }
    Ok(pages)
}

/// Model (b): greedy in stored order; a new page when the span reaches 2^W or the page is full.
fn layout_greedy(rels: &[[u64; 3]], width: u32, capacity: usize) -> Vec<Page> {
    let span = 1_u64 << width;
    let mut pages = Vec::new();
    let mut start = 0;
    let (mut min, mut max) = ([u64::MAX; 3], [0_u64; 3]);
    for (index, rel) in rels.iter().enumerate() {
        let next_min: [u64; 3] = std::array::from_fn(|axis| min[axis].min(rel[axis]));
        let next_max: [u64; 3] = std::array::from_fn(|axis| max[axis].max(rel[axis]));
        let fits =
            index - start < capacity && (0..3).all(|axis| next_max[axis] - next_min[axis] < span);
        if fits || index == start {
            (min, max) = (next_min, next_max);
        } else {
            pages.push(Page {
                start,
                end: index,
                origin: min,
            });
            start = index;
            (min, max) = (*rel, *rel);
        }
    }
    if start < rels.len() {
        pages.push(Page {
            start,
            end: rels.len(),
            origin: min,
        });
    }
    pages
}

fn check_pages(pages: &[Page], cells: usize, capacity: usize) -> Result<(), String> {
    let mut next = 0;
    for page in pages {
        if page.start != next || page.end <= page.start || page.end - page.start > capacity {
            return Err(format!(
                "page {}..{} breaks contiguity or capacity",
                page.start, page.end
            ));
        }
        next = page.end;
    }
    if next != cells {
        return Err(format!("pages cover {next} of {cells} cells"));
    }
    Ok(())
}

fn flags_of(cell: &NodeCell) -> u32 {
    u32::from(cell.count == 1)
        | u32::from(cell.intensity_count > 0) << 1
        | u32::from(cell.classification != -1) << 2
}

/// Packs x, y, z (W bits each) then optional flags (3 bits), little-endian across `u32` words.
fn encode_cell(out: &mut Vec<u32>, local: [u64; 3], flags: u32, combo: Combo) {
    let w = combo.width;
    let mut value =
        u128::from(local[0]) | u128::from(local[1]) << w | u128::from(local[2]) << (2 * w);
    if combo.flags_in_instance {
        value |= u128::from(flags) << (3 * w);
    }
    for word in 0..position_words(w, combo.flags_in_instance) {
        out.push((value >> (32 * word)) as u32);
    }
}

/// Field read with `u32` operations only, as WGSL would do it.
fn decode_field(words: &[u32], bit_offset: u32, width: u32) -> u32 {
    let index = (bit_offset / 32) as usize;
    let shift = bit_offset % 32;
    let mut value = words[index] >> shift;
    let taken = 32 - shift;
    if width > taken {
        value |= words[index + 1] << taken;
    }
    if width < 32 {
        value &= (1_u32 << width) - 1;
    }
    value
}

fn decode_cell(words: &[u32], combo: Combo) -> ([u32; 3], u32) {
    let w = combo.width;
    let local = [
        decode_field(words, 0, w),
        decode_field(words, w, w),
        decode_field(words, 2 * w, w),
    ];
    let flags = if combo.flags_in_instance {
        decode_field(words, 3 * w, 3)
    } else {
        0
    };
    (local, flags)
}

fn local_of(rel: [u64; 3], page: &Page) -> [u64; 3] {
    std::array::from_fn(|axis| rel[axis] - page.origin[axis])
}

fn encode_pages(
    cells: &[NodeCell],
    rels: &[[u64; 3]],
    pages: &[Page],
    combo: Combo,
    out: &mut Vec<u32>,
) {
    out.clear();
    for page in pages {
        for index in page.start..page.end {
            encode_cell(
                out,
                local_of(rels[index], page),
                flags_of(&cells[index]),
                combo,
            );
        }
    }
}

fn verify_decode(
    cells: &[NodeCell],
    rels: &[[u64; 3]],
    pages: &[Page],
    combo: Combo,
    words: &[u32],
) -> Result<(), String> {
    let stride = position_words(combo.width, combo.flags_in_instance);
    for page in pages {
        for index in page.start..page.end {
            let (local, flags) = decode_cell(&words[index * stride..(index + 1) * stride], combo);
            let expected = local_of(rels[index], page);
            let flags_ok = !combo.flags_in_instance || flags == flags_of(&cells[index]);
            if local.map(u64::from) != expected || !flags_ok {
                return Err(format!(
                    "decode mismatch W={} F={} cell {index}: {local:?}/{flags} vs {expected:?}",
                    combo.width,
                    combo.flag_name()
                ));
            }
        }
    }
    Ok(())
}

/// Strict Morton order of relative coordinates (axis 0 is the most significant of each triple).
fn morton_less(a: [u64; 3], b: [u64; 3]) -> bool {
    let mut best: Option<(u32, usize)> = None;
    for axis in 0..3 {
        let diff = a[axis] ^ b[axis];
        if diff != 0 {
            let bit = 63 - diff.leading_zeros();
            if best.is_none_or(|(top, _)| bit > top) {
                best = Some((bit, axis));
            }
        }
    }
    best.is_some_and(|(_, axis)| a[axis] < b[axis])
}

#[derive(Default, Clone)]
struct Agg {
    levels: u64,
    cells: u64,
    pages: u64,
    hist: [u64; HIST_BUCKETS],
}

impl Agg {
    fn add(&mut self, pages: &[Page]) {
        self.levels += 1;
        self.pages += pages.len() as u64;
        for page in pages {
            let count = page.end - page.start;
            self.cells += count as u64;
            let bucket = (usize::BITS - count.leading_zeros()) as usize;
            self.hist[bucket.min(HIST_BUCKETS - 1)] += 1;
        }
    }
}

struct Stats {
    /// Indexed by `(combo * 2 + model) * GROUPS.len() + group`.
    agg: Vec<Agg>,
    mask_pages: [u64; COMBOS],
    mask_bits: [u64; COMBOS],
    f32_failures: [u64; WIDTHS.len()],
    bits_hist: [[u64; 65]; 3],
    encode_ns: [[u128; REPS]; COMBOS],
    build: Duration,
    levels: u64,
    cells: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        return Err("usage: region_width_spike INPUT.oct [MAX_NODES=0]".into());
    }
    let max_nodes = args
        .get(1)
        .map(|value| value.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let mut attrs_path = args[0].clone();
    attrs_path.push("attrs");
    if !reader.has_attributes() {
        reader.attach_attributes(File::open(attrs_path)?)?;
    }
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
            return Err("node traversal mismatch between hierarchy and paged traversal".into());
        }
    }

    let combos = Combo::all();
    let source = PointNodeCellSource::default();
    let mut stats = Stats {
        agg: vec![Agg::default(); COMBOS * 2 * GROUPS.len()],
        mask_pages: [0; COMBOS],
        mask_bits: [0; COMBOS],
        f32_failures: [0; WIDTHS.len()],
        bits_hist: [[0; 65]; 3],
        encode_ns: [[0; REPS]; COMBOS],
        build: Duration::ZERO,
        levels: 0,
        cells: 0,
    };
    let mut buffer = Vec::new();
    let mut node_count = 0_u64;
    let selected = if max_nodes == 0 {
        nodes.len()
    } else {
        max_nodes.min(nodes.len())
    };
    for node in &nodes[..selected] {
        let bounds = node.bounds();
        let local = SpatialBounds::new(
            std::array::from_fn(|axis| bounds.min()[axis] - origin[axis]),
            std::array::from_fn(|axis| bounds.max()[axis] - origin[axis]),
        )?;
        let blocks: Vec<_> = node
            .blocks()
            .iter()
            .map(|id| reader.read_block(*id))
            .collect::<Result<_, _>>()?;
        let is_leaf = if leaf_flags.is_empty() {
            hierarchy.is_leaf(node.id())
        } else {
            *leaf_flags
                .get(&node.id())
                .ok_or("node missing authoritative has_children flag")?
        };
        let started = Instant::now();
        let built = source
            .node_cells(&grid, node.id(), local, node.spacing(), is_leaf, &blocks)
            .map_err(|error| format!("node {}: {error}", node.id().get()))?;
        stats.build += started.elapsed();
        node_count += 1;
        let level_count = built.levels.len();
        for (position, level) in built.levels.iter().enumerate() {
            let class = match (is_leaf, position + 1 == level_count) {
                (false, _) => 0,
                (true, false) => 1,
                (true, true) => 2,
            };
            let mut groups = vec![class];
            if position == 0 {
                groups.push(3);
            }
            if position + 1 == level_count {
                groups.push(4);
            }
            process_level(
                &mut stats,
                &mut buffer,
                &combos,
                &grid,
                local,
                level.depth,
                &level.cells,
                class,
                &groups,
            )
            .map_err(|error| format!("node {} depth {}: {error}", node.id().get(), level.depth))?;
        }
    }

    report(
        &args[0].to_string_lossy(),
        &args[0],
        node_count,
        &stats,
        &combos,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_level(
    stats: &mut Stats,
    buffer: &mut Vec<u32>,
    combos: &[Combo; COMBOS],
    grid: &CellGrid,
    local: SpatialBounds,
    depth: u8,
    cells: &[NodeCell],
    class: usize,
    groups: &[usize],
) -> Result<(), String> {
    let level_index = stats.levels;
    stats.levels += 1;
    stats.cells += cells.len() as u64;
    if cells.is_empty() {
        return Ok(());
    }
    let node_origin = node_cell_origin(grid, local, depth);
    let rels: Vec<[u64; 3]> = cells
        .iter()
        .map(|cell| {
            let mut rel = [0; 3];
            for axis in 0..3 {
                rel[axis] = cell.coord[axis]
                    .checked_sub(node_origin[axis])
                    .ok_or("cell below node origin")?;
            }
            Ok(rel)
        })
        .collect::<Result<_, String>>()?;
    if rels.windows(2).any(|pair| !morton_less(pair[0], pair[1])) {
        return Err("cells are not in strict Morton order of relative coordinates".into());
    }
    let max_rel = rels.iter().flatten().copied().max().unwrap_or(0);
    let bits = 64 - max_rel.leading_zeros();
    stats.bits_hist[class][bits as usize] += 1;
    if bits > BITS_ALARM {
        println!("# ALARM: relative bits {bits} > {BITS_ALARM} (depth {depth})");
    }

    let mut greedy_pages: Vec<Vec<Page>> = Vec::with_capacity(COMBOS);
    for (ci, combo) in combos.iter().enumerate() {
        let capacity = combo.capacity();
        let w = combo.width;
        let aligned = layout_aligned(&rels, w, capacity)?;
        let greedy = layout_greedy(&rels, w, capacity);
        for (model, pages) in [&aligned, &greedy].into_iter().enumerate() {
            check_pages(pages, cells.len(), capacity)?;
            for &group in groups {
                stats.agg[(ci * 2 + model) * GROUPS.len() + group].add(pages);
            }
        }
        // Every local must fit in W bits and in f32 with the 1/8-cell tolerance.
        for page in &aligned {
            for rel in &rels[page.start..page.end] {
                if local_of(*rel, page).iter().any(|&value| value >> w != 0) {
                    return Err("aligned local exceeds 2^W".into());
                }
            }
        }
        let mut mask_bits = 0_u64;
        let mut f32_failed = false;
        for page in &greedy {
            let mut mask = 0_u64;
            for rel in &rels[page.start..page.end] {
                let value = local_of(*rel, page);
                if value.iter().any(|&v| v >> w != 0) {
                    return Err("greedy local exceeds 2^W".into());
                }
                let brick = value.map(|v| v >> (w - 2));
                mask |= 1 << (brick[0] + 4 * brick[1] + 16 * brick[2]);
                f32_failed |= value
                    .iter()
                    .any(|&v| ((v as f32 + 0.5) as f64 - (v as f64 + 0.5)).abs() >= 0.125);
            }
            mask_bits += u64::from(mask.count_ones());
        }
        stats.mask_pages[ci] += greedy.len() as u64;
        stats.mask_bits[ci] += mask_bits;
        if !combo.flags_in_instance && f32_failed {
            stats.f32_failures[ci / 2] += 1;
        }
        greedy_pages.push(greedy);
    }

    // Encoding time: model (b) only, REPS passes with the combo order rotated by level index.
    #[allow(clippy::needless_range_loop)]
    for rep in 0..REPS {
        for k in 0..COMBOS {
            let ci = (k + level_index as usize) % COMBOS;
            let started = Instant::now();
            encode_pages(cells, &rels, &greedy_pages[ci], combos[ci], buffer);
            black_box(&*buffer);
            let elapsed = started.elapsed().as_nanos();
            stats.encode_ns[ci][rep] += elapsed;
            if rep == 0 {
                verify_decode(cells, &rels, &greedy_pages[ci], combos[ci], buffer)?;
            }
        }
    }
    Ok(())
}

fn report(
    path: &str,
    file: &std::ffi::OsStr,
    nodes: u64,
    stats: &Stats,
    combos: &[Combo; COMBOS],
) -> Result<(), Box<dyn std::error::Error>> {
    println!("# file\t{path}");
    println!("# file_size_bytes\t{}", std::fs::metadata(file)?.len());
    println!("# nodes\t{nodes}");
    println!("# levels\t{}", stats.levels);
    println!("# cells\t{}", stats.cells);
    println!("# page_bytes\t{PAGE_BYTES}\tpage_entry_bytes\t{PAGE_ENTRY_BYTES}");

    println!("# block layout");
    println!(
        "W\tF\tmodel\tgroup\tlevels\tcells\tpages\tmean_fill\tinstance_bytes\tinstance_bytes_total\ttail_bytes\tpage_entry_bytes\tbytes_per_cell"
    );
    for (ci, combo) in combos.iter().enumerate() {
        for (model, model_name) in MODELS.iter().enumerate() {
            for (group, group_name) in GROUPS.iter().enumerate() {
                let agg = &stats.agg[(ci * 2 + model) * GROUPS.len() + group];
                let ib = combo.instance_bytes() as u64;
                let total = agg.cells * ib;
                let tail = agg.pages * PAGE_BYTES as u64 - total;
                let entries = agg.pages * PAGE_ENTRY_BYTES as u64;
                let fill = agg.cells as f64 / (agg.pages * combo.capacity() as u64).max(1) as f64;
                let per_cell =
                    (agg.pages * PAGE_BYTES as u64 + entries) as f64 / agg.cells.max(1) as f64;
                println!(
                    "{}\t{}\t{model_name}\t{group_name}\t{}\t{}\t{}\t{fill:.6}\t{ib}\t{total}\t{tail}\t{entries}\t{per_cell:.4}",
                    combo.width,
                    combo.flag_name(),
                    agg.levels,
                    agg.cells,
                    agg.pages
                );
            }
        }
    }

    println!("# block hist_cells_per_page (bucket b = cells in [2^(b-1), 2^b), b0 unused)");
    let buckets: Vec<String> = (0..HIST_BUCKETS).map(|b| format!("b{b}")).collect();
    println!("W\tF\tmodel\tgroup\t{}", buckets.join("\t"));
    for (ci, combo) in combos.iter().enumerate() {
        for (model, model_name) in MODELS.iter().enumerate() {
            for (group, group_name) in GROUPS.iter().enumerate() {
                let agg = &stats.agg[(ci * 2 + model) * GROUPS.len() + group];
                let counts: Vec<String> = agg.hist.iter().map(u64::to_string).collect();
                println!(
                    "{}\t{}\t{model_name}\t{group_name}\t{}",
                    combo.width,
                    combo.flag_name(),
                    counts.join("\t")
                );
            }
        }
    }

    println!("# block max_relative_bits (levels per bit count, per class)");
    println!("class\tbits\tlevels\talarm");
    for (class, hist) in stats.bits_hist.iter().enumerate() {
        for (bits, &levels) in hist.iter().enumerate().filter(|&(_, &levels)| levels > 0) {
            println!(
                "{}\t{bits}\t{levels}\t{}",
                GROUPS[class],
                u8::from(bits as u32 > BITS_ALARM)
            );
        }
    }

    println!("# block encode_time (model b_greedy; build_ns = total time in node_cells)");
    println!("W\tF\trep\tencode_ns\tbuild_ns\tshare_of_build");
    let build_ns = stats.build.as_nanos();
    for (ci, combo) in combos.iter().enumerate() {
        for (rep, &ns) in stats.encode_ns[ci].iter().enumerate() {
            println!(
                "{}\t{}\t{rep}\t{ns}\t{build_ns}\t{:.6}",
                combo.width,
                combo.flag_name(),
                ns as f64 / build_ns.max(1) as f64
            );
        }
    }

    println!("# block mask4 (model b_greedy; mean fraction of occupied 4x4x4 sub-bricks per page)");
    println!("W\tF\tpages\tmean_occupied_fraction");
    for (ci, combo) in combos.iter().enumerate() {
        println!(
            "{}\t{}\t{}\t{:.6}",
            combo.width,
            combo.flag_name(),
            stats.mask_pages[ci],
            stats.mask_bits[ci] as f64 / (64 * stats.mask_pages[ci]).max(1) as f64
        );
    }

    println!("# block f32_failures (pages with a local whose f32 centre is off by >= 1/8 cell)");
    println!("W\tfailed_levels\tstatus");
    for (wi, width) in WIDTHS.iter().enumerate() {
        let failed = stats.f32_failures[wi];
        println!(
            "{width}\t{failed}\t{}",
            if failed == 0 { "ok" } else { "FAILED" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(width: u32) -> Vec<[u64; 3]> {
        let max = (1_u64 << width) - 1;
        let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ u64::from(width);
        let mut out = vec![
            [0; 3],
            [max; 3],
            [max, 0, max],
            [1 << (width - 1), 1, max - 1],
        ];
        for _ in 0..200 {
            let mut next = || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 20) & max
            };
            out.push([next(), next(), next()]);
        }
        out
    }

    #[test]
    fn instance_bytes_formula() {
        assert_eq!(instance_bytes(10, false), 12);
        assert_eq!(instance_bytes(10, true), 16);
        assert_eq!(instance_bytes(8, true), 12);
        assert_eq!(instance_bytes(32, false), 20);
        assert_eq!(instance_bytes(32, true), 24);
    }

    #[test]
    fn encode_decode_round_trip_for_every_width_and_flag_placement() {
        for combo in Combo::all() {
            for local in synthetic(combo.width) {
                for flags in 0..8 {
                    let mut words = Vec::new();
                    encode_cell(&mut words, local, flags, combo);
                    assert_eq!(
                        words.len(),
                        position_words(combo.width, combo.flags_in_instance)
                    );
                    let (decoded, decoded_flags) = decode_cell(&words, combo);
                    assert_eq!(decoded.map(u64::from), local);
                    if combo.flags_in_instance {
                        assert_eq!(decoded_flags, flags);
                    }
                }
            }
        }
    }

    #[test]
    fn greedy_splits_on_range() {
        let rels = [[0, 0, 0], [255, 0, 0], [256, 0, 0], [300, 5, 0]];
        let pages = layout_greedy(&rels, 8, 100);
        assert_eq!(
            pages,
            vec![
                Page {
                    start: 0,
                    end: 2,
                    origin: [0, 0, 0]
                },
                Page {
                    start: 2,
                    end: 4,
                    origin: [256, 0, 0]
                },
            ]
        );
    }

    #[test]
    fn greedy_splits_on_capacity() {
        let rels: Vec<[u64; 3]> = (0..5).map(|x| [x, 0, 0]).collect();
        let pages = layout_greedy(&rels, 10, 2);
        let spans: Vec<_> = pages.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(spans, vec![(0, 2), (2, 4), (4, 5)]);
        check_pages(&pages, 5, 2).unwrap();
    }

    #[test]
    fn aligned_blocks_split_on_key_and_capacity() {
        let rels = [[0, 0, 0], [1, 0, 0], [2, 0, 0], [256, 0, 0]];
        let pages = layout_aligned(&rels, 8, 2).unwrap();
        let spans: Vec<_> = pages.iter().map(|p| (p.start, p.end, p.origin)).collect();
        assert_eq!(
            spans,
            vec![(0, 2, [0; 3]), (2, 3, [0; 3]), (3, 4, [256, 0, 0])]
        );
    }

    #[test]
    fn morton_order_compares_highest_differing_bit_first() {
        assert!(morton_less([0, 0, 0], [0, 0, 1]));
        assert!(morton_less([0, 0, 1], [0, 1, 0]));
        assert!(morton_less([0, 1, 1], [1, 0, 0]));
        assert!(!morton_less([1, 0, 0], [0, 1, 1]));
        assert!(!morton_less([2, 2, 2], [2, 2, 2]));
    }
}
