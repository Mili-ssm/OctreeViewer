use std::{
    collections::{BTreeMap, HashMap},
    env,
    fs::File,
    path::Path,
};

use las::{PointDataBuilder, Reader as LasReader};
use viewer_core::{DatasetSource, NodeId, SpatialBounds, SpatialNode};
use viewer_format::FormatReader;

const LOD_GRID: usize = 8;
const MAX_SAMPLE_POINTS: u64 = 1_000_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if !(2..=3).contains(&arguments.len()) {
        eprintln!("Usage: spatial_quality_bench SOURCE DATASET.oct [--metadata-only]");
        return Err("expected source and converted dataset".into());
    }
    let metadata_only = arguments
        .get(2)
        .is_some_and(|argument| argument == "--metadata-only");
    if arguments.len() == 3 && !metadata_only {
        return Err("unknown option".into());
    }
    analyze(
        Path::new(&arguments[0]),
        Path::new(&arguments[1]),
        metadata_only,
    )
}

fn analyze(
    source: &Path,
    dataset: &Path,
    metadata_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut source_reader = LasReader::from_path(source)?;
    let source_header = source_reader.header().clone();
    let source_bounds = source_header.bounds();
    let source_min = [
        source_bounds.min.x,
        source_bounds.min.y,
        source_bounds.min.z,
    ];
    let source_max = [
        source_bounds.max.x,
        source_bounds.max.y,
        source_bounds.max.z,
    ];
    let root_region = cube_bounds(SpatialBounds::new(source_min, source_max)?);
    let source_count = source_header.number_of_points();

    let mut reader = FormatReader::new(File::open(dataset)?)?;
    let format_metadata = reader.format_metadata().clone();
    let hierarchy = format_metadata.dataset().hierarchy();
    let origin = format_metadata.dataset().coordinate_frame().origin();
    if hierarchy.stats().source_point_count != Some(source_count) {
        return Err(format!(
            "source point count {} does not match dataset metadata {:?}",
            source_count,
            hierarchy.stats().source_point_count
        )
        .into());
    }
    if origin != source_min {
        return Err("source bounds origin does not match the converted dataset".into());
    }
    if hierarchy.stats().max_level < 3 {
        return Err(format!(
            "dataset hierarchy reaches level {}, but comparison requires at least level 3; convert with a smaller --points-per-block value",
            hierarchy.stats().max_level
        )
        .into());
    }

    let nodes = hierarchy
        .query(hierarchy.stats().root_bounds)
        .collect::<Vec<_>>();
    let mut children = HashMap::<NodeId, Vec<NodeId>>::new();
    let mut node_by_id = HashMap::<NodeId, &SpatialNode>::new();
    for node in &nodes {
        node_by_id.insert(node.id(), node);
        if let Some(parent) = node.parent() {
            children.entry(parent).or_default().push(node.id());
        }
    }
    for child_nodes in children.values_mut() {
        child_nodes.sort_unstable();
    }

    let mut regions = HashMap::new();
    let root = hierarchy.root_id();
    regions.insert(root, root_region);
    let mut pending = vec![root];
    let mut child_keys = HashMap::<(NodeId, usize), NodeId>::new();
    while let Some(parent_id) = pending.pop() {
        let Some(parent_region) = regions.get(&parent_id).copied() else {
            continue;
        };
        let Some(child_ids) = children.get(&parent_id) else {
            continue;
        };
        for child_id in child_ids {
            let child = node_by_id[child_id];
            let key = child_index(child.bounds(), parent_region);
            let region = child_bounds(parent_region, key);
            if child_keys.insert((parent_id, key), *child_id).is_some() {
                return Err(format!("two child nodes occupy octant {key} of {parent_id:?}").into());
            }
            regions.insert(*child_id, region);
            pending.push(*child_id);
        }
    }

    let storage = reader.block_storage_stats()?;
    let mut block_owner = HashMap::new();
    for node in &nodes {
        for block_id in node.blocks() {
            if block_owner.insert(*block_id, node.id()).is_some() {
                return Err(format!("block {block_id:?} is referenced by multiple nodes").into());
            }
        }
    }

    let mut representatives = HashMap::<(NodeId, usize), [f64; 3]>::new();
    for node in &nodes {
        if node.point_count() == 0 || children.contains_key(&node.id()) {
            for block_id in node.blocks() {
                let block = reader.read_block(*block_id)?;
                let region = regions[&node.id()];
                for point in block.points().points() {
                    let world =
                        std::array::from_fn(|axis| f64::from(point.position[axis]) + origin[axis]);
                    let cell = sample_index(world, region);
                    if representatives.insert((node.id(), cell), world).is_some() {
                        return Err(format!(
                            "duplicate LOD representative for node {:?}, cell {cell}",
                            node.id()
                        )
                        .into());
                    }
                }
            }
        }
    }

    let strides = [1_u32, 2, 3];
    let retained_stats = strides
        .iter()
        .map(|stride| {
            let retained_nodes = nodes
                .iter()
                .filter(|node| !children.contains_key(&node.id()) || node.level() % stride == 0)
                .count();
            let retained_blocks = storage
                .iter()
                .filter(|stats| {
                    let node_id = block_owner[&stats.id];
                    let node = node_by_id[&node_id];
                    !children.contains_key(&node_id) || node.level().is_multiple_of(*stride)
                })
                .collect::<Vec<_>>();
            let payload_bytes = retained_blocks
                .iter()
                .map(|stats| stats.stored_bytes)
                .sum::<u64>();
            let raw_payload_bytes = retained_blocks
                .iter()
                .map(|stats| stats.raw_bytes)
                .sum::<u64>();
            let point_count = retained_blocks
                .iter()
                .map(|stats| stats.point_count)
                .sum::<u64>();
            (
                *stride,
                retained_nodes,
                retained_blocks.len(),
                payload_bytes,
                raw_payload_bytes,
                point_count,
            )
        })
        .collect::<Vec<_>>();

    if metadata_only {
        for (stride, node_count, block_count, payload_bytes, raw_payload_bytes, point_count) in
            retained_stats
        {
            println!(
                "stride={stride} retained_nodes={node_count} retained_blocks={block_count} retained_payload_bytes={payload_bytes} retained_raw_payload_bytes={raw_payload_bytes} retained_compression_ratio={:.6} retained_payload_points={point_count}",
                payload_bytes as f64 / raw_payload_bytes.max(1) as f64
            );
        }
        let original_payload = storage.iter().map(|stats| stats.stored_bytes).sum::<u64>();
        let original_file_bytes = std::fs::metadata(dataset)?.len();
        println!("original_file_bytes={original_file_bytes}");
        println!("original_payload_bytes={original_payload}");
        println!(
            "original_index_and_header_bytes={}",
            original_file_bytes - original_payload
        );
        return Ok(());
    }

    let sample_stride = source_count.div_ceil(MAX_SAMPLE_POINTS).max(1);
    let mut metrics = strides
        .iter()
        .map(|stride| (*stride, QualityMetrics::default()))
        .collect::<BTreeMap<_, _>>();
    let mut point_data = PointDataBuilder::new()
        .for_header(source_reader.header())
        .build();
    let mut point_index = 0_u64;
    while point_index < source_count {
        let read =
            source_reader.fill_points(262_144.min(source_count - point_index), &mut point_data)?;
        if read == 0 {
            return Err(format!(
                "source ended after {point_index} points; expected {source_count}"
            )
            .into());
        }
        for ((x, y), z) in point_data.x().zip(point_data.y()).zip(point_data.z()) {
            if stable_hash(point_index).is_multiple_of(sample_stride) {
                let position = [x, y, z];
                let mut selected = root;
                let mut cut_positions = BTreeMap::new();
                for stride in strides {
                    while node_by_id[&selected].level() < stride {
                        if !children.contains_key(&selected) {
                            break;
                        }
                        let region = regions[&selected];
                        let key = child_index_position(position, midpoint(region));
                        let Some(child_id) = child_keys.get(&(selected, key)) else {
                            let known = child_keys
                                .keys()
                                .filter_map(|(parent, child_key)| {
                                    (*parent == selected).then_some(*child_key)
                                })
                                .collect::<Vec<_>>();
                            return Err(format!("source point maps to missing child {key} under {selected:?}; known children are {known:?}; point={position:?}; region={region:?}").into());
                        };
                        selected = *child_id;
                    }

                    let node = node_by_id[&selected];
                    let represented =
                        if children.contains_key(&selected) {
                            let cell = sample_index(position, regions[&selected]);
                            representatives.get(&(selected, cell)).copied().ok_or_else(|| {
                            format!("missing LOD representative for node {selected:?}, cell {cell}")
                        })?
                        } else {
                            position
                        };
                    cut_positions.insert(stride, represented);
                    let residual = std::array::from_fn(|axis| represented[axis] - position[axis]);
                    metrics
                        .get_mut(&stride)
                        .expect("known stride")
                        .record(residual, node.geometric_error());
                }
                for pair in [(1, 2), (2, 3)] {
                    let left = cut_positions[&pair.0];
                    let right = cut_positions[&pair.1];
                    let delta = std::array::from_fn(|axis| right[axis] - left[axis]);
                    metrics
                        .get_mut(&pair.1)
                        .expect("known stride")
                        .record_transition(delta);
                }
            }
            point_index += 1;
        }
    }

    println!("source_points={source_count}");
    println!("sample_points={}", metrics[&1].samples);
    println!("sample_stride={sample_stride}");
    println!("sampling=stable_hash(source_point_index) % sample_stride == 0");
    println!("lod_grid={LOD_GRID}x{LOD_GRID}x{LOD_GRID}");
    for (stride, node_count, block_count, payload_bytes, raw_payload_bytes, point_count) in
        retained_stats
    {
        let metric = &metrics[&stride];
        println!(
            "stride={stride} equivalent_divisions_per_axis={} retained_nodes={node_count} retained_blocks={block_count} retained_payload_bytes={payload_bytes} retained_raw_payload_bytes={raw_payload_bytes} retained_compression_ratio={:.6} retained_payload_points={point_count}",
            1_u32 << stride,
            payload_bytes as f64 / raw_payload_bytes.max(1) as f64
        );
        println!(
            "stride={stride} residual_mean_xyz={:.6},{:.6},{:.6} residual_rmse={:.6} residual_p95={:.6} residual_max_observed={:.6} declared_error_max={:.6} declared_error_violations={}",
            metric.mean_residual()[0],
            metric.mean_residual()[1],
            metric.mean_residual()[2],
            metric.rmse(),
            metric.percentile(0.95),
            metric.max_error,
            metric.max_declared_error,
            metric.declared_error_violations,
        );
        if stride > 1 {
            println!(
                "stride={stride} transition_mean_xyz={:.6},{:.6},{:.6} transition_p95={:.6} transition_max_observed={:.6}",
                metric.mean_transition()[0],
                metric.mean_transition()[1],
                metric.mean_transition()[2],
                metric.transition_percentile(0.95),
                metric.max_transition,
            );
        }
    }
    let original_payload = storage.iter().map(|stats| stats.stored_bytes).sum::<u64>();
    let original_file_bytes = std::fs::metadata(dataset)?.len();
    println!("original_file_bytes={original_file_bytes}");
    println!("original_payload_bytes={original_payload}");
    println!(
        "original_index_and_header_bytes={}",
        original_file_bytes - original_payload
    );
    Ok(())
}

#[derive(Default)]
struct QualityMetrics {
    samples: u64,
    residual_sum: [f64; 3],
    squared_error_sum: f64,
    errors: Vec<f64>,
    max_error: f64,
    max_declared_error: f64,
    declared_error_violations: u64,
    transition_sum: [f64; 3],
    transitions: Vec<f64>,
    max_transition: f64,
}

impl QualityMetrics {
    fn record(&mut self, residual: [f64; 3], declared_error: f64) {
        let error = length(residual);
        self.samples += 1;
        for (sum, value) in self.residual_sum.iter_mut().zip(residual) {
            *sum += value;
        }
        self.squared_error_sum += error * error;
        self.errors.push(error);
        self.max_error = self.max_error.max(error);
        self.max_declared_error = self.max_declared_error.max(declared_error);
        if error > declared_error + 1e-5 {
            self.declared_error_violations += 1;
        }
    }

    fn record_transition(&mut self, delta: [f64; 3]) {
        for (sum, value) in self.transition_sum.iter_mut().zip(delta) {
            *sum += value;
        }
        let distance = length(delta);
        self.transitions.push(distance);
        self.max_transition = self.max_transition.max(distance);
    }

    fn mean_residual(&self) -> [f64; 3] {
        mean(self.residual_sum, self.samples)
    }

    fn mean_transition(&self) -> [f64; 3] {
        mean(self.transition_sum, self.transitions.len() as u64)
    }

    fn rmse(&self) -> f64 {
        (self.squared_error_sum / self.samples.max(1) as f64).sqrt()
    }

    fn percentile(&self, fraction: f64) -> f64 {
        percentile(&self.errors, fraction)
    }

    fn transition_percentile(&self, fraction: f64) -> f64 {
        percentile(&self.transitions, fraction)
    }
}

fn mean(sum: [f64; 3], count: u64) -> [f64; 3] {
    std::array::from_fn(|axis| sum[axis] / count.max(1) as f64)
}

fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * fraction).ceil() as usize;
    sorted[index]
}

fn stable_hash(value: u64) -> u64 {
    let mut value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn length(vector: [f64; 3]) -> f64 {
    vector.iter().map(|value| value * value).sum::<f64>().sqrt()
}

fn cube_bounds(bounds: SpatialBounds) -> SpatialBounds {
    let min = bounds.min();
    let side = (0..3)
        .map(|axis| bounds.max()[axis] - min[axis])
        .fold(0.0, f64::max);
    let max = std::array::from_fn(|axis| (min[axis] + side).max(bounds.max()[axis]));
    SpatialBounds::new(min, max).expect("source bounds are finite and ordered")
}

fn child_index(bounds: SpatialBounds, parent: SpatialBounds) -> usize {
    let center = std::array::from_fn::<_, 3, _>(|axis| {
        bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) / 2.0
    });
    child_index_position(center, midpoint(parent))
}

fn midpoint(bounds: SpatialBounds) -> [f64; 3] {
    std::array::from_fn(|axis| bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) / 2.0)
}

fn child_index_position(position: [f64; 3], middle: [f64; 3]) -> usize {
    (usize::from(position[0] >= middle[0]) << 2)
        | (usize::from(position[1] >= middle[1]) << 1)
        | usize::from(position[2] >= middle[2])
}

fn child_bounds(bounds: SpatialBounds, child: usize) -> SpatialBounds {
    let middle = std::array::from_fn::<_, 3, _>(|axis| {
        bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) / 2.0
    });
    let min = std::array::from_fn(|axis| {
        if child & (1 << (2 - axis)) == 0 {
            bounds.min()[axis]
        } else {
            middle[axis]
        }
    });
    let max = std::array::from_fn(|axis| {
        if child & (1 << (2 - axis)) == 0 {
            middle[axis]
        } else {
            bounds.max()[axis]
        }
    });
    SpatialBounds::new(min, max).expect("child partition lies within its parent")
}

fn sample_index(position: [f64; 3], bounds: SpatialBounds) -> usize {
    let side = bounds.max()[0] - bounds.min()[0];
    let mut cell = [0; 3];
    for axis in 0..3 {
        let scaled = ((position[axis] - bounds.min()[axis]) / side * LOD_GRID as f64).floor();
        cell[axis] = (scaled as usize).min(LOD_GRID - 1);
    }
    (cell[0] * LOD_GRID + cell[1]) * LOD_GRID + cell[2]
}
