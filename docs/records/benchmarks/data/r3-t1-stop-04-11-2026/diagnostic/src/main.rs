use std::{collections::{BTreeMap, HashSet},fs::File};
use viewer_core::DatasetSource;
use viewer_format::FormatReader;
use viewer_render_wgpu::{CellGrid, CHAIN_SPAN, representative_depth, representative_depth_for_spacing};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).unwrap();
    let mut reader = FormatReader::new(File::open(&path)?)?;
    let mut attrs = path.clone(); attrs.push("attrs");
    if !reader.has_attributes() { reader.attach_attributes(File::open(attrs)?)?; }
    let dataset = reader.format_metadata().viewer_dataset()?;
    let hierarchy = dataset.hierarchy().clone();
    let root = hierarchy.stats().root_bounds;
    let origin = dataset.coordinate_frame().origin();
    let root_side = (0..3).map(|a| root.max()[a]-root.min()[a]).fold(0.0, f64::max);
    let grid = CellGrid {root_min: std::array::from_fn(|a| root.min()[a]-origin[a]), root_side};
    let mut leaves=0; let mut flagged=0; let mut duplicate_only=0; let mut distinct_collision=0; let mut nonfinite=0; let mut samples=0;
    for node in hierarchy.query(root) {
        if !hierarchy.is_leaf(node.id()) {continue;}
        leaves+=1;
        let bounds=node.bounds();
        let local=viewer_core::SpatialBounds::new(std::array::from_fn(|a| bounds.min()[a]-origin[a]), std::array::from_fn(|a| bounds.max()[a]-origin[a]))?;
        let first=node.spacing().map_or_else(|| representative_depth(&grid, local), |s| representative_depth_for_spacing(&grid,s));
        let depth=first+CHAIN_SPAN;
        if depth>=64 {return Err(format!("absolute depth {depth}").into());}
        let count=1u64<<depth; let cell_side=grid.root_side/count as f64;
        let range:[(u64,u64);3]=std::array::from_fn(|a| { let edge=|v:f64| ((v-grid.root_min[a])/cell_side).round(); let last=(count-1) as f64; let lo=edge(local.min()[a]).clamp(0.,last); let hi=(edge(local.max()[a])-1.).clamp(lo,last); (lo as u64,hi as u64) });
        let mut bins=BTreeMap::<[u64;3],[u32;3]>::new(); let mut unique=HashSet::new(); let mut n=0usize; let mut finite=0usize; let mut collision=None;
        for id in node.blocks() { let block=reader.read_block(*id)?; for point in block.points().points() { n+=1; if point.position.iter().any(|v| !v.is_finite()) { nonfinite+=1; continue; } finite+=1;
            let bits=point.position.map(|v| if v==0.0 {0} else {v.to_bits()}); unique.insert(bits);
            let cell=std::array::from_fn(|a| (((f64::from(point.position[a])-grid.root_min[a])/grid.root_side*count as f64).floor().clamp(range[a].0 as f64,range[a].1 as f64)) as u64);
            if let Some(prev)=bins.insert(cell,bits) { if prev!=bits && collision.is_none() {collision=Some((cell,prev,bits));} }
        }}
        if bins.len()<n {flagged+=1; if bins.len()<unique.len() { distinct_collision+=1; if samples<12 { samples+=1; let (cell,a,b)=collision.unwrap(); println!("distinct node={} level={} first={} cap={} node_side={} spacing={:?} total={} finite={} unique={} cells={} local_min={:?} local_max={:?} cell={:?} a={:?} b={:?} bits_a={:?} bits_b={:?}",node.id().get(),node.level(),first,depth,local.max()[0]-local.min()[0],node.spacing(),n,finite,unique.len(),bins.len(),local.min(),local.max(),cell,a.map(f32::from_bits),b.map(f32::from_bits),a,b); } } else {duplicate_only+=1; if samples<3 {samples+=1;println!("duplicates node={} level={} first={} cap={} total={} finite={} unique={} cells={}",node.id().get(),node.level(),first,depth,n,finite,unique.len(),bins.len());} } }
    }
    println!("leaves={leaves} flagged={flagged} duplicate_or_nonfinite_only={duplicate_only} distinct_collision={distinct_collision} nonfinite={nonfinite} root_side={root_side}"); Ok(())
}
