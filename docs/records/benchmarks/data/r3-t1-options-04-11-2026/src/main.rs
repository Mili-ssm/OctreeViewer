use std::{collections::HashSet,fs::File};
use viewer_core::DatasetSource;
use viewer_format::FormatReader;
use viewer_render_wgpu::{CellGrid, CHAIN_SPAN, CHAIN_MIN_GROWTH, chain_keep_mask, representative_depth, representative_depth_for_spacing, NodeCell};
fn cells(p:&[[f32;3]], grid:&CellGrid, local:viewer_core::SpatialBounds, d:u8)->usize {
 let count=1u64<<d; let side=grid.root_side/count as f64;
 let range:[(u64,u64);3]=std::array::from_fn(|a| {let edge=|v:f64| ((v-grid.root_min[a])/side).round();let last=(count-1) as f64;let lo=edge(local.min()[a]).clamp(0.,last);let hi=(edge(local.max()[a])-1.).clamp(lo,last);(lo as u64,hi as u64)});
 p.iter().map(|pt| std::array::from_fn::<_,3,_>(|a| (((f64::from(pt[a])-grid.root_min[a])/grid.root_side*count as f64).floor().clamp(range[a].0 as f64,range[a].1 as f64)) as u64)).collect::<HashSet<_>>().len()
}
fn kept(c:&[usize])->usize {chain_keep_mask(c,CHAIN_MIN_GROWTH).iter().zip(c).filter_map(|(k,n)|k.then_some(*n)).sum()}
fn main()->Result<(),Box<dyn std::error::Error>> {
 let path=std::env::args_os().nth(1).unwrap();let mut reader=FormatReader::new(File::open(&path)?)?;let mut attrs=path.clone();attrs.push("attrs");if !reader.has_attributes(){reader.attach_attributes(File::open(attrs)?)?;}
 let dataset=reader.format_metadata().viewer_dataset()?;let hierarchy=dataset.hierarchy().clone();let root=hierarchy.stats().root_bounds;let origin=dataset.coordinate_frame().origin();let root_side=(0..3).map(|a|root.max()[a]-root.min()[a]).fold(0.0,f64::max);let grid=CellGrid{root_min:std::array::from_fn(|a|root.min()[a]-origin[a]),root_side};
 let(mut flagged,mut nf,mut sum_fixed,mut sum_adapt)=(0,0,0,0);
 println!("NodeCell size={} bytes; pruning growth={}",std::mem::size_of::<NodeCell>(),CHAIN_MIN_GROWTH);
 for node in hierarchy.query(root){if !hierarchy.is_leaf(node.id()){continue;}let b=node.bounds();let local=viewer_core::SpatialBounds::new(std::array::from_fn(|a|b.min()[a]-origin[a]),std::array::from_fn(|a|b.max()[a]-origin[a]))?;let first=node.spacing().map_or_else(||representative_depth(&grid,local),|s|representative_depth_for_spacing(&grid,s));let base=representative_depth(&grid,local)-3;let mut p=Vec::new();for id in node.blocks(){let block=reader.read_block(*id)?;for point in block.points().points(){if point.position.iter().all(|v|v.is_finite()){p.push(point.position)}else{nf+=1;}}}let unique=p.iter().map(|v|v.map(|x|if x==0.0{0}else{x.to_bits()})).collect::<HashSet<_>>().len();let cap=first+CHAIN_SPAN;let ncap=cells(&p,&grid,local,cap);if ncap>=unique{continue;}flagged+=1;let mut counts=(first..=cap).map(|d|cells(&p,&grid,local,d)).collect::<Vec<_>>();let fixed=kept(&counts);let mut separate=None;for d in cap+1..=base+21 {let n=cells(&p,&grid,local,d);counts.push(n);if n==unique {separate=Some(d);break;}}let adaptive=kept(&counts);sum_fixed+=fixed;sum_adapt+=adaptive;println!("node={} first={} node_base={} cap={} total={} unique={} duplicates={} cap_cells={} separate_abs={:?} separate_relative={:?} counts={:?} fixed_kept={} adaptive_kept={} extra={}",node.id().get(),first,base,cap,p.len(),unique,p.len()-unique,ncap,separate,separate.map(|d|d-base),counts,fixed,adaptive,adaptive as i64-fixed as i64);
 }
 println!("flagged={flagged} nonfinite={nf} fixed_cells={sum_fixed} adaptive_cells={sum_adapt} extra_cells={} fixed_bytes={} adaptive_bytes={} extra_bytes={}",sum_adapt as i64-sum_fixed as i64,sum_fixed*std::mem::size_of::<NodeCell>(),sum_adapt*std::mem::size_of::<NodeCell>(),(sum_adapt as i64-sum_fixed as i64)*std::mem::size_of::<NodeCell>() as i64);Ok(())
}
