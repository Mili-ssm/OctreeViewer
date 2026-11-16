use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use viewer_core::{
    AttributeValues, BlockId, DatasetSource, NodeId, Point, PointBlock, SpatialHierarchy,
};
use viewer_format::{FormatReader, OctreeReader};

const DEFAULT_REPETITIONS: usize = 3;
const DEFAULT_BUDGET_MIB: u64 = 512;
const DEFAULT_WORKERS: usize = 1;
const MAX_WORKERS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Hierarchy,
    Reconstruct,
    Progressive,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(args.next().ok_or(
        "usage: storage_bench INPUT.oct hierarchy|reconstruct|progressive [REPETITIONS [MAX_DECODED_MIB]] [--workers N]",
    )?);
    let mode = match args.next().as_deref() {
        Some("hierarchy") => Mode::Hierarchy,
        Some("reconstruct") => Mode::Reconstruct,
        Some("progressive") => Mode::Progressive,
        _ => return Err("mode must be hierarchy, reconstruct, or progressive".into()),
    };
    let mut positional = Vec::new();
    let mut workers = DEFAULT_WORKERS;
    let mut workers_was_set = false;
    while let Some(argument) = args.next() {
        if argument == "--workers" {
            if workers_was_set {
                return Err("--workers may be specified only once".into());
            }
            workers_was_set = true;
            workers = args
                .next()
                .ok_or("--workers requires a positive integer from 1 to 8")?
                .parse()?;
        } else {
            positional.push(argument);
        }
    }
    let repetitions = positional
        .first()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(DEFAULT_REPETITIONS);
    let budget_mib = positional
        .get(1)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(DEFAULT_BUDGET_MIB);
    if positional.len() > 2
        || repetitions == 0
        || budget_mib == 0
        || !(1..=MAX_WORKERS).contains(&workers)
        || (mode != Mode::Progressive && workers_was_set)
    {
        return Err(
            "repetitions and decoded budget must be positive; --workers must be from 1 to 8 and applies only to progressive mode".into(),
        );
    }

    let oct_bytes = fs::metadata(&input)?.len();
    let attrs_path = attributes_path(&input);
    let (attrs_present, attrs_bytes) = match fs::metadata(&attrs_path) {
        Ok(metadata) => (true, metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, 0),
        Err(error) => return Err(error.into()),
    };
    for repetition in 1..=repetitions {
        let full_start = Instant::now();
        let open_start = Instant::now();
        let mut reader = FormatReader::new(File::open(&input)?)?;
        let open_ms = millis(open_start.elapsed());
        let stats = reader.block_storage_stats()?;
        let payloads = reader.block_payload_storage_stats()?;
        let constant_payloads = payloads
            .iter()
            .filter(|p| p.codec == viewer_format::BlockCodec::Constant)
            .count();
        let dictionary_payloads = payloads
            .iter()
            .filter(|p| p.codec == viewer_format::BlockCodec::Dictionary)
            .count();
        let attach_start = Instant::now();
        if mode != Mode::Hierarchy && attrs_present {
            reader.attach_attributes(File::open(&attrs_path)?)?;
        }
        if mode != Mode::Hierarchy && !reader.has_attributes() {
            return Err("complete-payload benchmark requires the .octattrs companion".into());
        }
        let attach_ms = millis(attach_start.elapsed());
        let block_estimates = estimate_block_bytes(&stats)?;
        let decoded_estimate = block_estimates
            .values()
            .try_fold(0_u64, |sum, size| sum.checked_add(*size))
            .ok_or("decoded estimate overflow")?;
        let max_block_estimate = block_estimates.values().copied().max().unwrap_or(0);
        let budget_bytes = budget_mib
            .checked_mul(1024 * 1024)
            .ok_or("budget overflow")?;
        let checked_estimate = budget_estimate(mode, decoded_estimate, max_block_estimate, workers);
        if mode != Mode::Hierarchy && checked_estimate > budget_bytes {
            return Err(format!(
                "decoded estimate {checked_estimate} exceeds budget {budget_bytes}; no payload read"
            )
            .into());
        }
        let base = format!(
            "rep={repetition} mode={} format_version={} header_envelope={} oct_bytes={oct_bytes} attrs_bytes={attrs_bytes} open_ms={open_ms:.3} attach_ms={attach_ms:.3} decoded_estimate_bytes={decoded_estimate} max_block_estimate_bytes={max_block_estimate} budget_estimate_bytes={checked_estimate} budget_basis=point_struct_plus_raw_block_payloads budget_bytes={budget_bytes} constant_payloads={constant_payloads} dictionary_payloads={dictionary_payloads}",
            mode_name(mode),
            reader.format_version(),
            viewer_format::HEADER_ENVELOPE_VERSION,
        );
        match mode {
            Mode::Hierarchy => println!(
                "{base} load_ms=0 full_ms={:.3} blocks={}",
                millis(full_start.elapsed()),
                stats.len()
            ),
            Mode::Reconstruct => {
                let load_start = Instant::now();
                let (count, retained) = reconstruct(&mut reader, &stats)?;
                let load_ms = millis(load_start.elapsed());
                let mut checksum = Checksum::default();
                let verify_start = Instant::now();
                for block in &retained {
                    hash_block(&mut checksum, block);
                }
                let verify_ms = millis(verify_start.elapsed());
                println!(
                    "{base} load_ms={load_ms:.3} verify_ms={verify_ms:.3} full_ms={:.3} blocks={} points={count} checksum={:016x}",
                    millis(full_start.elapsed()),
                    stats.len(),
                    checksum.finish()
                );
            }
            Mode::Progressive => {
                let hierarchy = reader.metadata().hierarchy();
                let traversal = make_traversal(hierarchy);
                let result = if workers == 1 {
                    progressive(&mut reader, &traversal, &block_estimates)?
                } else {
                    progressive_parallel(
                        &reader,
                        &input,
                        &attrs_path,
                        &traversal,
                        &block_estimates,
                        workers,
                    )?
                };
                println!(
                    "{base} workers_limit={workers} worker_threads={} read_ms={:.3} progressive_wall_ms={:.3} verify_ms={:.3} full_ms={:.3} blocks={} points={} checksum={:016x} first_root_read_ms={:.3}",
                    result.worker_threads,
                    result.read_ms,
                    result.progressive_wall_ms,
                    result.verify_ms,
                    millis(full_start.elapsed()),
                    result.blocks,
                    result.points,
                    result.checksum,
                    result.root_read_ms
                );
                for level in result.levels {
                    println!(
                        "rep={repetition} level={} nodes={} blocks={} points={} node_read_latency_p50_ms={:.3} node_read_latency_p95_ms={:.3}",
                        level.level,
                        level.nodes,
                        level.blocks,
                        level.points,
                        millis(level.p50),
                        millis(level.p95)
                    );
                }
            }
        }
    }
    Ok(())
}

fn attributes_path(input: &Path) -> PathBuf {
    input.with_extension("octattrs")
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Hierarchy => "hierarchy",
        Mode::Reconstruct => "reconstruct",
        Mode::Progressive => "progressive",
    }
}

fn estimate_block_bytes(
    stats: &[viewer_format::BlockStorageStats],
) -> Result<BTreeMap<BlockId, u64>, Box<dyn std::error::Error>> {
    // Conservative payload preflight adds decoded Point storage to all raw block payloads,
    // including geometry already represented in Points. Vec metadata, allocator overhead,
    // and transient decompression buffers are excluded.
    let mut estimates = BTreeMap::new();
    for stat in stats {
        let points = stat
            .point_count
            .checked_mul(std::mem::size_of::<Point>() as u64)
            .ok_or("decoded point estimate overflow")?;
        estimates.insert(
            stat.id,
            points
                .checked_add(stat.raw_bytes)
                .ok_or("decoded raw-block estimate overflow")?,
        );
    }
    Ok(estimates)
}

fn budget_estimate(mode: Mode, total: u64, max_block: u64, workers: usize) -> u64 {
    if mode == Mode::Progressive {
        max_block.saturating_mul(workers as u64)
    } else {
        total
    }
}

fn reconstruct<R: OctreeReader>(
    reader: &mut R,
    stats: &[viewer_format::BlockStorageStats],
) -> Result<(u64, Vec<PointBlock>), Box<dyn std::error::Error>>
where
    R::Error: std::error::Error + 'static,
{
    let mut blocks = Vec::with_capacity(stats.len());
    let mut count = 0_u64;
    for stat in stats {
        let block = reader.read_block(stat.id)?;
        if block.id() != stat.id || block.points().len() as u64 != stat.point_count {
            return Err(format!("block {} ID or point count mismatch", stat.id.get()).into());
        }
        count += stat.point_count;
        blocks.push(block);
    }
    Ok((count, blocks))
}

struct LevelStats {
    level: u32,
    nodes: usize,
    blocks: usize,
    points: u64,
    p50: Duration,
    p95: Duration,
}

struct ProgressiveStats {
    levels: Vec<LevelStats>,
    blocks: usize,
    points: u64,
    checksum: u64,
    root_read_ms: f64,
    progressive_wall_ms: f64,
    read_ms: f64,
    verify_ms: f64,
    worker_threads: usize,
}

#[derive(Clone)]
struct TraversalNode {
    id: NodeId,
    level: u32,
    point_count: u64,
    blocks: Vec<BlockId>,
}

fn make_traversal(hierarchy: &SpatialHierarchy) -> Vec<TraversalNode> {
    let mut traversal = Vec::new();
    for id in bfs_ids(hierarchy.root_id(), |id| hierarchy.child_ids(id).to_vec()) {
        if let Some(node) = hierarchy.node(id) {
            traversal.push(TraversalNode {
                id,
                level: node.level(),
                point_count: node.point_count(),
                blocks: node.blocks().to_vec(),
            });
        }
    }
    traversal
}

fn bfs_ids(root: NodeId, mut children: impl FnMut(NodeId) -> Vec<NodeId>) -> Vec<NodeId> {
    let mut pending = VecDeque::from([root]);
    let mut ordered = Vec::new();
    while let Some(id) = pending.pop_front() {
        ordered.push(id);
        pending.extend(children(id));
    }
    ordered
}

fn progressive<R: OctreeReader>(
    reader: &mut R,
    traversal: &[TraversalNode],
    block_estimates: &BTreeMap<BlockId, u64>,
) -> Result<ProgressiveStats, Box<dyn std::error::Error>>
where
    R::Error: std::error::Error + 'static,
{
    let mut levels: Vec<LevelStats> = Vec::new();
    let mut checksum = Checksum::default();
    let mut total_blocks = 0;
    let mut total_points = 0;
    let mut root_read_ms = 0.0;
    let progressive_start = Instant::now();
    let mut load_duration = Duration::ZERO;
    let mut verify_duration = Duration::ZERO;
    let mut samples: Vec<(u32, Duration)> = Vec::new();
    for (node_index, node) in traversal.iter().enumerate() {
        let mut node_blocks = 0;
        let mut node_points = 0_u64;
        let mut node_read_duration = Duration::ZERO;
        for block_id in &node.blocks {
            let _one_live_block_budget = block_estimates
                .get(block_id)
                .ok_or("missing block estimate")?;
            let read_start = Instant::now();
            let block = reader.read_block(*block_id)?;
            let read_elapsed = read_start.elapsed();
            load_duration += read_elapsed;
            node_read_duration += read_elapsed;
            if block.id() != *block_id {
                return Err(format!("block {} ID mismatch", block_id.get()).into());
            }
            node_points += block.points().len() as u64;
            let verify_start = Instant::now();
            hash_block(&mut checksum, &block);
            verify_duration += verify_start.elapsed();
            node_blocks += 1;
            total_blocks += 1;
        }
        if node_points != node.point_count {
            return Err(format!(
                "node {} point count mismatch: metadata {}, read {node_points}",
                node.id.get(),
                node.point_count
            )
            .into());
        }
        if node_index == 0 {
            root_read_ms = millis(node_read_duration);
        }
        total_points += node_points;
        let level = node.level;
        samples.push((level, node_read_duration));
        match levels.last_mut() {
            Some(entry) if entry.level == level => {
                entry.nodes += 1;
                entry.blocks += node_blocks;
                entry.points += node_points;
            }
            _ => levels.push(LevelStats {
                level,
                nodes: 1,
                blocks: node_blocks,
                points: node_points,
                p50: node_read_duration,
                p95: node_read_duration,
            }),
        }
    }
    for entry in &mut levels {
        let mut durations = samples
            .iter()
            .filter(|(level, _)| *level == entry.level)
            .map(|(_, duration)| *duration)
            .collect::<Vec<_>>();
        (entry.p50, entry.p95) = percentiles(&mut durations);
    }
    Ok(ProgressiveStats {
        levels,
        blocks: total_blocks,
        points: total_points,
        checksum: checksum.finish(),
        root_read_ms,
        progressive_wall_ms: millis(progressive_start.elapsed()),
        read_ms: millis(load_duration),
        verify_ms: millis(verify_duration),
        worker_threads: 0,
    })
}

fn progressive_parallel(
    reader: &FormatReader<File>,
    input: &Path,
    attrs_path: &Path,
    traversal: &[TraversalNode],
    block_estimates: &BTreeMap<BlockId, u64>,
    workers: usize,
) -> Result<ProgressiveStats, Box<dyn std::error::Error>> {
    let mut levels: Vec<LevelStats> = Vec::new();
    let mut samples: Vec<(u32, Duration)> = Vec::new();
    let mut checksum = Checksum::default();
    let mut total_blocks = 0;
    let mut total_points = 0;
    let mut read_duration = Duration::ZERO;
    let mut verify_duration = Duration::ZERO;
    let mut root_read = Duration::ZERO;
    let worker_threads = workers;
    let progressive_start = Instant::now();
    let jobs = traversal
        .iter()
        .enumerate()
        .flat_map(|(node_index, node)| {
            node.blocks
                .iter()
                .map(move |block_id| (node_index, *block_id))
        })
        .collect::<Vec<_>>();
    let mut node_index = 0;
    let mut node_blocks = 0;
    let mut node_points = 0_u64;
    let mut node_read = Duration::ZERO;
    let mut forks = Vec::with_capacity(workers);
    for _ in 0..workers {
        let mut fork = reader.fork_reader(File::open(input)?);
        fork.attach_attributes(File::open(attrs_path)?)?;
        forks.push(fork);
    }
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::mpsc::sync_channel;

        let mut job_senders = Vec::with_capacity(workers);
        let mut result_receivers = Vec::with_capacity(workers);
        let mut handles = Vec::with_capacity(workers);
        for mut fork in forks {
            let (job_tx, job_rx) = sync_channel::<BlockId>(0);
            let (result_tx, result_rx) =
                sync_channel::<Result<(Duration, PointBlock), viewer_format::FormatError>>(0);
            job_senders.push(job_tx);
            result_receivers.push(result_rx);
            handles.push(scope.spawn(move || {
                while let Ok(block_id) = job_rx.recv() {
                    let start = Instant::now();
                    let result = fork
                        .read_block(block_id)
                        .map(|block| (start.elapsed(), block));
                    if result_tx.send(result).is_err() {
                        break;
                    }
                }
            }));
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            for batch in jobs.chunks(workers) {
                for (worker_index, (_, block_id)) in batch.iter().enumerate() {
                    job_senders[worker_index]
                        .send(*block_id)
                        .map_err(|_| "progressive worker job channel closed")?;
                }
                for ((job_node_index, block_id), result_receiver) in
                    batch.iter().zip(&result_receivers)
                {
                    let (read_elapsed, block) = result_receiver
                        .recv()
                        .map_err(|_| "progressive worker result channel closed")??;
                    while node_index < *job_node_index {
                        finish_parallel_node(
                            node_index,
                            node_blocks,
                            node_points,
                            node_read,
                            traversal,
                            &mut levels,
                            &mut samples,
                        )?;
                        if node_index == 0 {
                            root_read = node_read;
                        }
                        total_points += node_points;
                        node_index += 1;
                        node_blocks = 0;
                        node_points = 0;
                        node_read = Duration::ZERO;
                    }
                    if block.id() != *block_id {
                        return Err(format!("block {} ID mismatch", block_id.get()).into());
                    }
                    let _one_live_block_budget = block_estimates
                        .get(block_id)
                        .ok_or("missing block estimate")?;
                    read_duration += read_elapsed;
                    node_read += read_elapsed;
                    node_points += block.points().len() as u64;
                    let verify_start = Instant::now();
                    hash_block(&mut checksum, &block);
                    verify_duration += verify_start.elapsed();
                    node_blocks += 1;
                    total_blocks += 1;
                }
            }
            Ok(())
        })();
        // Closing coordinator endpoints lets workers leave recv/send before joining,
        // including when dispatch or aggregation returns an error.
        drop(job_senders);
        drop(result_receivers);
        for handle in handles {
            handle
                .join()
                .map_err(|_| "progressive reader worker panicked")?;
        }
        result
    })?;
    while node_index < traversal.len() {
        finish_parallel_node(
            node_index,
            node_blocks,
            node_points,
            node_read,
            traversal,
            &mut levels,
            &mut samples,
        )?;
        if node_index == 0 {
            root_read = node_read;
        }
        total_points += node_points;
        node_index += 1;
        node_blocks = 0;
        node_points = 0;
        node_read = Duration::ZERO;
    }
    for entry in &mut levels {
        let mut durations = samples
            .iter()
            .filter(|(level, _)| *level == entry.level)
            .map(|(_, duration)| *duration)
            .collect::<Vec<_>>();
        (entry.p50, entry.p95) = percentiles(&mut durations);
    }
    Ok(ProgressiveStats {
        levels,
        blocks: total_blocks,
        points: total_points,
        checksum: checksum.finish(),
        root_read_ms: millis(root_read),
        progressive_wall_ms: millis(progressive_start.elapsed()),
        read_ms: millis(read_duration),
        verify_ms: millis(verify_duration),
        worker_threads,
    })
}

fn finish_parallel_node(
    node_index: usize,
    blocks: usize,
    points: u64,
    read: Duration,
    traversal: &[TraversalNode],
    levels: &mut Vec<LevelStats>,
    samples: &mut Vec<(u32, Duration)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let node = &traversal[node_index];
    if points != node.point_count {
        return Err(format!(
            "node {} point count mismatch: metadata {}, read {points}",
            node.id.get(),
            node.point_count
        )
        .into());
    }
    samples.push((node.level, read));
    match levels.last_mut() {
        Some(entry) if entry.level == node.level => {
            entry.nodes += 1;
            entry.blocks += blocks;
            entry.points += points;
        }
        _ => levels.push(LevelStats {
            level: node.level,
            nodes: 1,
            blocks,
            points,
            p50: read,
            p95: read,
        }),
    }
    Ok(())
}

struct Checksum(u64);

impl Default for Checksum {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Checksum {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn finish(self) -> u64 {
        self.0
    }
}

fn hash_block(checksum: &mut Checksum, block: &PointBlock) {
    checksum.write(&block.id().get().to_le_bytes());
    for point in block.points().points() {
        for coordinate in point.position {
            checksum.write(&coordinate.to_bits().to_le_bytes());
        }
        checksum.write(&[point.color.r, point.color.g, point.color.b]);
    }
    for column in block.attributes() {
        let name = column.descriptor().name().as_bytes();
        checksum.write(&(name.len() as u64).to_le_bytes());
        checksum.write(name);
        hash_values(checksum, column.values());
    }
}

fn hash_values(checksum: &mut Checksum, values: &AttributeValues) {
    checksum.write(&[match values {
        AttributeValues::U8(_) => 0,
        AttributeValues::I8(_) => 1,
        AttributeValues::U16(_) => 2,
        AttributeValues::I16(_) => 3,
        AttributeValues::U32(_) => 4,
        AttributeValues::I32(_) => 5,
        AttributeValues::U64(_) => 6,
        AttributeValues::I64(_) => 7,
        AttributeValues::F32(_) => 8,
        AttributeValues::F64(_) => 9,
    }]);
    macro_rules! hash_vec {
        ($v:expr) => {
            for value in $v {
                checksum.write(&value.to_le_bytes());
            }
        };
    }
    match values {
        AttributeValues::U8(v) => checksum.write(v),
        AttributeValues::I8(v) => hash_vec!(v),
        AttributeValues::U16(v) => hash_vec!(v),
        AttributeValues::I16(v) => hash_vec!(v),
        AttributeValues::U32(v) => hash_vec!(v),
        AttributeValues::I32(v) => hash_vec!(v),
        AttributeValues::U64(v) => hash_vec!(v),
        AttributeValues::I64(v) => hash_vec!(v),
        AttributeValues::F32(v) => {
            for value in v {
                checksum.write(&value.to_bits().to_le_bytes());
            }
        }
        AttributeValues::F64(v) => {
            for value in v {
                checksum.write(&value.to_bits().to_le_bytes());
            }
        }
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn percentiles(values: &mut [Duration]) -> (Duration, Duration) {
    values.sort_unstable();
    let index = |p: usize| (values.len() * p).div_ceil(100).saturating_sub(1);
    (values[index(50)], values[index(95)])
}

#[cfg(test)]
mod tests {
    use super::{Mode, bfs_ids, budget_estimate, percentiles};
    use std::{collections::HashMap, time::Duration};
    use viewer_core::NodeId;

    #[test]
    fn bfs_follows_edges_instead_of_identifier_order() {
        let edges = HashMap::from([
            (NodeId::new(90), vec![NodeId::new(30), NodeId::new(70)]),
            (NodeId::new(30), vec![NodeId::new(10)]),
            (NodeId::new(70), vec![NodeId::new(20)]),
        ]);
        let actual = bfs_ids(NodeId::new(90), |id| {
            edges.get(&id).cloned().unwrap_or_default()
        });
        assert_eq!(actual, [90, 30, 70, 10, 20].map(NodeId::new));
    }

    #[test]
    fn progressive_budget_uses_largest_live_block() {
        assert_eq!(budget_estimate(Mode::Progressive, 900, 300, 1), 300);
        assert_eq!(budget_estimate(Mode::Progressive, 900, 300, 4), 1200);
        assert_eq!(budget_estimate(Mode::Reconstruct, 900, 300, 4), 900);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut samples = [Duration::from_millis(2), Duration::from_millis(1)];
        assert_eq!(
            percentiles(&mut samples),
            (Duration::from_millis(1), Duration::from_millis(2))
        );
    }
}
