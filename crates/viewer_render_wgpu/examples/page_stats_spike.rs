//! Throwaway P0 spike of ADR 0030: page statistics of a v7.4-pre hierarchy if it were paged every
//! k levels, plus the pages touched by the initial-view LOD cut. Synthetic mode: `--synthetic`.
use std::{collections::HashMap, fs::File};

use viewer_core::{AttributeDataType, FrustumPlane, LodSelectionConfig, NodeId, SpatialFrustum};
use viewer_format::FormatReader;
use viewer_render_wgpu::{OrbitCamera, SceneBounds};

const PAGE_HEADER: u64 = 1 + 24 + 4 + 4; // level, prefix, crc32, node count
const POINTER: u64 = 8 + 4;

struct Tree {
    parent: Vec<u32>, // u32::MAX for the root
    depth: Vec<u32>,
    children: Vec<u32>,
    slim: Vec<u64>, // slim bytes per node
    id: Vec<u64>,
}

fn width(t: AttributeDataType) -> u64 {
    use AttributeDataType::{F32, F64, I8, I16, I32, I64, U8, U16, U32, U64};
    match t {
        U8 | I8 => 1,
        U16 | I16 => 2,
        U32 | I32 | F32 => 4,
        U64 | I64 | F64 => 8,
    }
}

fn pct(sorted: &[u64], p: f64) -> u64 {
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

/// `page_of`[n]: anchor index of the page holding n's record; the root's page is the root itself.
fn page_of(tree: &Tree, k: u32) -> Vec<u32> {
    let mut page = vec![0_u32; tree.parent.len()];
    // nodes are stored parent-before-child
    for n in 1..tree.parent.len() {
        let p = tree.parent[n] as usize;
        page[n] = if tree.depth[p].is_multiple_of(k) {
            p as u32
        } else {
            page[p]
        };
    }
    page
}

fn page_stats(tree: &Tree, k: u32, label: &str, extra: &serde_json::Value) -> (Vec<u32>, Vec<u64>) {
    let page = page_of(tree, k);
    let mut bytes: HashMap<u32, (u64, u64)> = HashMap::new(); // anchor -> (nodes, bytes)
    for n in 0..tree.parent.len() {
        let e = bytes.entry(page[n]).or_insert((0, PAGE_HEADER));
        e.0 += 1;
        e.1 += tree.slim[n];
        // node at relative depth k (page's last level) with children carries a child-page pointer
        if tree.depth[n] > 0 && tree.depth[n].is_multiple_of(k) && tree.children[n] > 0 {
            e.1 += POINTER;
        }
    }
    let mut nodes: Vec<u64> = bytes.values().map(|v| v.0).collect();
    let mut sz: Vec<u64> = bytes.values().map(|v| v.1).collect();
    nodes.sort_unstable();
    sz.sort_unstable();
    let total: u64 = sz.iter().sum();
    let over = |limit: u64| sz.iter().filter(|b| **b > limit).count();
    let over_bytes =
        |limit: u64| sz.iter().filter(|b| **b > limit).sum::<u64>() as f64 / total as f64;
    println!(
        "{}",
        serde_json::json!({
            "dataset": label, "k": k, "pages": sz.len(),
            "nodes_p50": pct(&nodes, 0.5), "nodes_p95": pct(&nodes, 0.95), "nodes_max": nodes[nodes.len() - 1],
            "bytes_p50": pct(&sz, 0.5), "bytes_p95": pct(&sz, 0.95), "bytes_max": sz[sz.len() - 1],
            "slim_total_bytes": total, "pages_over_512k": over(512 * 1024), "bytes_share_over_512k": over_bytes(512 * 1024), "pages_over_1m": over(1 << 20), "extra": extra,
        })
    );
    (page, sz_by_anchor(&bytes, tree.parent.len()))
}

fn sz_by_anchor(bytes: &HashMap<u32, (u64, u64)>, n: usize) -> Vec<u64> {
    let mut v = vec![0_u64; n];
    for (a, b) in bytes {
        v[*a as usize] = b.1;
    }
    v
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--synthetic") {
        return synthetic(&args[1..]);
    }
    let path = args
        .first()
        .ok_or("usage: page_stats_spike INPUT.oct | --synthetic")?;
    let label = std::path::Path::new(path)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let reader = FormatReader::new(File::open(path)?)?;
    let fm = reader.format_metadata();
    let ds = fm.viewer_dataset()?;
    let h = ds.hierarchy();
    let schema = h.attributes();
    let cols = 2 + schema.len() as u64;
    let blocks: HashMap<u64, u64> = reader
        .block_storage_stats()?
        .iter()
        .map(|b| (b.id.get(), b.point_count))
        .collect();
    let _ = blocks;
    // traverse parent-before-child
    let mut tree = Tree {
        parent: vec![],
        depth: vec![],
        children: vec![],
        slim: vec![],
        id: vec![],
    };
    let root = h.root_id();
    let a0 = h.node(root).unwrap().level();
    let mut idx_of: HashMap<u64, u32> = HashMap::new();
    let mut queue = std::collections::VecDeque::from([(root, u32::MAX)]);
    let (mut cur_total, mut ranges_bytes, mut block_n) = (0_u64, 0_u64, 0_u64);
    let (mut leaf_slim, mut leaf_n, mut int_slim, mut int_n) = (0_u64, 0_u64, 0_u64, 0_u64);
    while let Some((id, par)) = queue.pop_front() {
        let node = h.node(id).unwrap();
        let i = tree.parent.len() as u32;
        idx_of.insert(id.get(), i);
        let nchild = h.child_ids(id).len() as u32;
        let leaf = nchild == 0;
        let nb = node.blocks().len() as u64;
        block_n += nb;
        let mut rb = 4_u64;
        for r in fm.node_attribute_ranges(id) {
            let d = &schema[r.schema_index() as usize];
            rb += 4 + 2 * width(d.data_type()) * u64::from(d.component_count());
        }
        ranges_bytes += rb;
        let cur = 8
            + if par == u32::MAX { 1 } else { 9 }
            + 4
            + 24
            + 48
            + 8
            + 8
            + 8
            + 4
            + 8 * nb
            + rb
            + nb * (70 + 29 * cols);
        cur_total += cur;
        // slim: depth 1, path 2, point_count 8, geometric_error 8, spacing 8 (leaves only),
        // block_count 4, ranges as today; per block: offset 8, point_count 8, enc 1, qerr 8,
        // origin 24, per column stored_len 8 + codec 1 + crc 4.
        let slim = 1
            + 2
            + 8
            + 8
            + if leaf { 8 } else { 0 }
            + 4
            + rb
            + nb * (8 + 8 + 1 + 8 + 24 + 13 * cols);
        if leaf {
            leaf_slim += slim;
            leaf_n += 1;
        } else {
            int_slim += slim;
            int_n += 1;
        }
        tree.parent.push(par);
        tree.depth.push(node.level() - a0);
        tree.children.push(nchild);
        tree.slim.push(slim);
        tree.id.push(id.get());
        for c in h.child_ids(id) {
            queue.push_back((*c, i));
        }
    }
    eprintln!(
        "{label}: nodes {} blocks {block_n} root level {a0} max depth {} index_size_bytes {} model_current_nodes+blocks {cur_total} ranges {ranges_bytes} leaf_slim_avg {} int_slim_avg {} schema_attrs {}",
        tree.parent.len(),
        tree.depth.iter().max().unwrap(),
        reader.index_size_bytes(),
        leaf_slim / leaf_n.max(1),
        int_slim / int_n.max(1),
        schema.len()
    );
    println!(
        "{}",
        serde_json::json!({"dataset": label, "nodes": tree.parent.len(), "blocks": block_n,
        "current_index_bytes": reader.index_size_bytes(), "model_current_bytes": cur_total,
        "leaf_slim_avg": leaf_slim / leaf_n.max(1), "internal_slim_avg": int_slim / int_n.max(1),
        "leaves": leaf_n, "internal": int_n, "max_depth": tree.depth.iter().max()})
    );

    // initial-view cut
    let origin = ds.coordinate_frame().origin();
    let rb = h.stats().root_bounds;
    let viewport = [1920, 1080];
    let mut camera = OrbitCamera::default();
    if !camera.fit(
        SceneBounds {
            min: std::array::from_fn(|a| rb.min()[a] - origin[a]),
            max: std::array::from_fn(|a| rb.max()[a] - origin[a]),
        },
        viewport,
    ) {
        return Err("fit".into());
    }
    let lf = camera.spatial_frustum(viewport).ok_or("frustum")?;
    let wf = SpatialFrustum::new(
        lf.planes()
            .iter()
            .map(|p| {
                FrustumPlane::new(
                    p.normal(),
                    p.offset() - (0..3).map(|a| p.normal()[a] * origin[a]).sum::<f64>(),
                )
            })
            .collect::<Result<_, _>>()?,
    )?;
    let mut cuts: Vec<(f64, u64, Vec<u32>)> = vec![];
    for target in [8.0, 4.0, 1.0, 0.5] {
        for budget in [4_000_000_u64, u64::MAX] {
            let cfg = LodSelectionConfig::new(target, budget)?;
            let sel = h.select_lod(&wf, cfg, |n: &viewer_core::SpatialNode| {
                camera.projected_error_pixels(
                    n.bounds(),
                    n.representative_spacing(),
                    origin,
                    viewport,
                )
            })?;
            cuts.push((
                target,
                budget,
                sel.node_ids()
                    .iter()
                    .map(|id: &NodeId| idx_of[&id.get()])
                    .collect(),
            ));
        }
    }
    for k in [3_u32, 4, 5] {
        let (page, sizes) = page_stats(&tree, k, &label, &serde_json::json!({}));
        let mut pdepth = vec![0_u32; tree.parent.len()];
        for n in 1..tree.parent.len() {
            if tree.depth[n].is_multiple_of(k) {
                pdepth[n] = pdepth[page[n] as usize] + 1;
            }
        }
        for (target, budget, sel) in &cuts {
            let mut touched = std::collections::BTreeSet::new();
            let mut touched_b = std::collections::BTreeSet::new();
            let mut seen = std::collections::HashSet::new();
            for &s in sel {
                let mut n = s;
                touched.insert(page[s as usize]);
                touched_b.insert(page[s as usize]);
                if tree.depth[s as usize] > 0
                    && tree.depth[s as usize].is_multiple_of(k)
                    && tree.children[s as usize] > 0
                {
                    touched_b.insert(s);
                }
                while n != u32::MAX && seen.insert(n) {
                    touched.insert(page[n as usize]);
                    touched_b.insert(page[n as usize]);
                    n = tree.parent[n as usize];
                }
            }
            let maxd = |s: &std::collections::BTreeSet<u32>| {
                s.iter().map(|a| pdepth[*a as usize]).max().unwrap_or(0)
            };
            let by = |s: &std::collections::BTreeSet<u32>| {
                s.iter().map(|a| sizes[*a as usize]).sum::<u64>()
            };
            let maxdepth_sel = sel
                .iter()
                .map(|s| tree.depth[*s as usize])
                .max()
                .unwrap_or(0);
            println!(
                "{}",
                serde_json::json!({"dataset": label, "k": k, "cut_target_px": target,
                "point_budget": if *budget == u64::MAX { "unlimited".into() } else { budget.to_string() },
                "selected_nodes": sel.len(), "selected_max_depth": maxdepth_sel,
                "pages_touched": touched.len(), "max_page_depth": maxd(&touched), "touched_bytes": by(&touched),
                "pages_touched_incl_selected_anchors": touched_b.len(), "max_page_depth_incl": maxd(&touched_b),
                "touched_bytes_incl": by(&touched_b)})
            );
        }
    }
    Ok(())
}

fn synthetic(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // args: leaf_slim internal_slim ; deterministic xorshift
    let leaf: u64 = args[0].parse()?;
    let internal: u64 = args[1].parse()?;
    let target = 6_000_000_usize;
    for (name, q) in [
        ("synthetic_branch4", 0.5_f64),
        ("synthetic_branch8_full", 1.0),
        ("synthetic_branch6", 0.75),
    ] {
        let mut s = 0x9E37_79B9_7F4A_7C15_u64;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1_u64 << 53) as f64
        };
        let mut tree = Tree {
            parent: vec![u32::MAX],
            depth: vec![0],
            children: vec![0],
            slim: vec![0],
            id: vec![0],
        };
        let mut head = 0;
        while head < tree.parent.len() {
            let mut c = 0;
            for _ in 0..8 {
                // root always has children; cap total nodes at the target (BFS frontier stays leaves)
                if (rnd() < q || head == 0) && tree.parent.len() < target {
                    tree.parent.push(head as u32);
                    tree.depth.push(tree.depth[head] + 1);
                    tree.children.push(0);
                    tree.slim.push(0);
                    tree.id.push(0);
                    c += 1;
                }
            }
            tree.children[head] = c;
            head += 1;
        }
        for n in 0..tree.parent.len() {
            tree.slim[n] = if tree.children[n] == 0 {
                leaf
            } else {
                internal
            };
        }
        let mean = tree.children.iter().map(|c| f64::from(*c)).sum::<f64>()
            / tree.children.iter().filter(|c| **c > 0).count() as f64;
        for k in [3_u32, 4, 5] {
            page_stats(
                &tree,
                k,
                name,
                &serde_json::json!({"nodes": tree.parent.len(), "max_depth": tree.depth.iter().max(),
                "mean_children_of_internal": mean, "slim_leaf": leaf, "slim_internal": internal}),
            );
        }
    }
    Ok(())
}
