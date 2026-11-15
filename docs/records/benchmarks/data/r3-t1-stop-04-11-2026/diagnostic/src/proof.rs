use std::fs::File;
use viewer_core::{DatasetSource, SpatialBounds};
use viewer_format::FormatReader;
use viewer_render_wgpu::{CellGrid, NodeCellSource, PointNodeCellSource};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("missing dataset")?;
    let mut reader = FormatReader::new(File::open(&path)?)?;
    let mut attrs = path.clone();
    attrs.push("attrs");
    if !reader.has_attributes() { reader.attach_attributes(File::open(attrs)?)?; }
    let dataset = reader.format_metadata().viewer_dataset()?;
    let hierarchy = dataset.hierarchy().clone();
    let root = hierarchy.stats().root_bounds;
    let origin = dataset.coordinate_frame().origin();
    let side = (0..3).map(|a| root.max()[a] - root.min()[a]).fold(0.0, f64::max);
    let grid = CellGrid { root_min: std::array::from_fn(|a| root.min()[a] - origin[a]), root_side: side };
    let node = hierarchy.query(root).find(|node| node.id().get() == 19).ok_or("node 19 absent")?;
    assert!(hierarchy.is_leaf(node.id()));
    let bounds = node.bounds();
    let local = SpatialBounds::new(std::array::from_fn(|a| bounds.min()[a] - origin[a]), std::array::from_fn(|a| bounds.max()[a] - origin[a]))?;
    let spacing = node.spacing().ok_or("missing spacing")?;
    let blocks = node.blocks().iter().map(|id| reader.read_block(*id)).collect::<Result<Vec<_>,_>>()?;
    let wanted = [[1142617866_u32, 1139024429, 1137472897], [1142616130_u32, 1139024200, 1137472864]];
    let points = wanted.map(|bits| blocks.iter().flat_map(|b| b.points().points()).find(|p| p.position.map(f32::to_bits) == bits).expect("expected point absent").position);
    let build = |spacing| PointNodeCellSource { min_growth: 1.0 }.node_cells(&grid,node.id(),local,Some(spacing),true,&blocks);
    let regular = build(spacing)?;
    // Request a chain starting one level finer solely to inspect the production builder at cap+1.
    let finer = build(spacing / 2.0)?;
    let at_cap = regular.levels.last().ok_or("empty regular chain")?;
    let beyond = finer.levels.iter().find(|level| level.depth == at_cap.depth+1).ok_or("cap+1 absent")?;
    let coords = |depth: u8| points.map(|p| std::array::from_fn::<_,3,_>(|a| (((f64::from(p[a])-grid.root_min[a])/grid.root_side)*((1_u64<<depth) as f64)).floor() as u64));
    let cap_coords = coords(at_cap.depth);
    let finer_coords = coords(beyond.depth);
    assert_eq!(cap_coords[0],cap_coords[1]);
    assert_ne!(finer_coords[0],finer_coords[1]);
    let shared = at_cap.cells.iter().find(|cell| cell.coord==cap_coords[0]).ok_or("shared cap cell absent")?;
    assert!(shared.count>=2);
    for coord in finer_coords { assert!(beyond.cells.iter().any(|cell| cell.coord == coord)); }
    println!("node={} leaf=true root_side={} node_side={} stored_spacing={spacing} regular_depths={:?}", node.id().get(), grid.root_side, local.max()[0]-local.min()[0], regular.levels.iter().map(|l| l.depth).collect::<Vec<_>>());
    println!("local_points={points:?} point_bits={wanted:?}");
    println!("cap_depth={} cap_coords={cap_coords:?} shared_cell_count={} saved_cells={}", at_cap.depth, shared.count, at_cap.cells.len());
    println!("cap_plus_one_depth={} coords={finer_coords:?} saved_cells={} (diagnostic spacing={})", beyond.depth, beyond.cells.len(), spacing/2.0);
    Ok(())
}
