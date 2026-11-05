use std::fs::File;
use viewer_core::DatasetSource;
use viewer_format::FormatReader;
use viewer_render_wgpu::{CellGrid, representative_depth_for_spacing};
fn main()->Result<(),Box<dyn std::error::Error>> {
 let path=std::env::args_os().nth(1).unwrap();let mut reader=FormatReader::new(File::open(&path)?)?;let mut attrs=path.clone();attrs.push("attrs");if !reader.has_attributes(){reader.attach_attributes(File::open(attrs)?)?;}
 let dataset=reader.format_metadata().viewer_dataset()?;let hierarchy=dataset.hierarchy().clone();let root=hierarchy.stats().root_bounds;let origin=dataset.coordinate_frame().origin();let root_side=(0..3).map(|a|root.max()[a]-root.min()[a]).fold(0.0,f64::max);let grid=CellGrid{root_min:std::array::from_fn(|a|root.min()[a]-origin[a]),root_side};
 for node in hierarchy.query(root) { if node.id().get()!=120259084303 {continue;}
 let b=node.bounds();let min:[f64;3]=std::array::from_fn(|a|b.min()[a]-origin[a]);let max:[f64;3]=std::array::from_fn(|a|b.max()[a]-origin[a]);let depth=representative_depth_for_spacing(&grid,node.spacing().unwrap());let count=1u64<<depth;let side=grid.root_side/count as f64;
 let ranges:[(u64,u64);3]=std::array::from_fn(|a|{let last=(count-1)as f64;let lo=((min[a]-grid.root_min[a])/side).round().clamp(0.,last);let hi=(((max[a]-grid.root_min[a])/side).round()-1.).clamp(lo,last);(lo as u64,hi as u64)});
 println!("id={} origin={:?} root_min={:?} local_min={:?} local_max={:?} depth={} cell_side={} ranges={:?}",node.id().get(),origin,grid.root_min,min,max,depth,side,ranges);
 for id in node.blocks(){let block=reader.read_block(*id)?;for point in block.points().points(){let coord:[u64;3]=std::array::from_fn(|a|(((f64::from(point.position[a])-grid.root_min[a])/grid.root_side*count as f64).floor().clamp(ranges[a].0 as f64,ranges[a].1 as f64))as u64);let relative:[u64;3]=std::array::from_fn(|a|coord[a]-ranges[a].0);let bits=relative.map(|x|64-x.leading_zeros());println!("position={:?} world={:?} coord={:?} relative={:?} bits={:?} finite={}",point.position,std::array::from_fn::<_,3,_>(|a|f64::from(point.position[a])+origin[a]),coord,relative,bits,point.position.iter().all(|v|v.is_finite()));}}
 } Ok(())
}