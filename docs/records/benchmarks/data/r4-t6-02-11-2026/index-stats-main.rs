use std::{collections::HashMap, fs::File};
use viewer_format::FormatReader;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: stats FILE.oct")?;
    let total = std::fs::metadata(&path)?.len();
    let reader = FormatReader::new(File::open(&path)?)?;
    let ds = reader.format_metadata().viewer_dataset()?;
    let h = ds.hierarchy();
    let st = h.stats();
    let nodes: Vec<_> = h.query(st.root_bounds).cloned().collect();
    let by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id(), n)).collect();
    let root = nodes.iter().find(|n| n.parent().is_none()).ok_or("no root")?;
    let leaves = nodes.iter().filter(|n| h.is_leaf(n.id())).count();
    let single = |n: &viewer_core::SpatialNode| h.child_ids(n.id()).len() == 1;
    let chain_nodes = nodes.iter().filter(|n| single(n)).count();
    let rb = root.bounds();
    let side = (0..3).map(|a| rb.max()[a] - rb.min()[a]).fold(0.0, f64::max);
    // longest run of consecutive single-child nodes
    let mut longest = 0usize;
    for n in &nodes {
        if !single(n) { continue; }
        let starts = n.parent().is_none_or(|p| !single(by_id[&p]));
        if !starts { continue; }
        let (mut len, mut cur): (usize, &viewer_core::SpatialNode) = (0, n);
        loop {
            if !single(cur) { break; }
            len += 1;
            cur = by_id[&h.child_ids(cur.id())[0]];
        }
        longest = longest.max(len);
    }
    let mut hist = std::collections::BTreeMap::<u32, (usize, usize)>::new();
    for n in &nodes { let e = hist.entry(n.level()).or_default(); e.0 += 1; e.1 += usize::from(single(n)); }
    println!("file {path}");
    println!("nodes {}", nodes.len());
    println!("stats_node_count {}", st.node_count);
    println!("leaves {leaves}");
    println!("max_level {}", st.max_level);
    println!("root_level {}", root.level());
    println!("root_side {side}");
    println!("single_child_nodes {chain_nodes}");
    println!("longest_single_child_run_nodes {longest}");
    println!("index_bytes {}", reader.index_size_bytes());
    println!("oct_bytes {total}");
    println!("index_fraction_of_oct {:.6}", reader.index_size_bytes() as f64 / total as f64);
    println!("level nodes single_child");
    for (l, (c, s)) in hist { println!("{l} {c} {s}"); }
    Ok(())
}
