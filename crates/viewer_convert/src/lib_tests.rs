use crate::publish::{
    OutputValidation, output_attributes_path, output_block_ids, validate_output_pair,
};
use std::{fs, path::PathBuf};
use viewer_convert_core::limits::visual_scale_for_bounds;

use las::{Builder, Color, Point, Writer, point::Format};
use viewer_convert_core::options::{DEFAULT_SPATIAL_WORKERS, MAX_SPATIAL_WORKERS};
use viewer_core::{AttributeValues, BlockId, DatasetSource, NodeId, SpatialBounds};
use viewer_format::{FormatReader, OctreeReader};

use super::{
    ConversionStage, ConvertError, ConvertOptions, convert_file, convert_file_with_diagnostics,
    convert_file_with_progress,
};
use crate::test_options::{leaf_budget, leaf_new};

mod conversion;
mod diagnostics;
mod pinned_output;
mod planner;
mod root_ranges;
pub(crate) mod serial_core;
mod spool_lifecycle;
mod workers;

fn test_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn fixture(path: &std::path::Path, compressed: bool, colored: bool, count: usize) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(if colored { 2 } else { 0 }).unwrap();
    builder.point_format.is_compressed = compressed;
    let header = builder.into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for index in 0..count {
        let mut point = Point {
            x: 1_000_000.0 + index as f64 * 0.25,
            y: 2_000_000.0 + (index % 3) as f64,
            z: 50.0 + (index % 5) as f64,
            intensity: (index as u16).wrapping_mul(10_000),
            return_number: 2,
            number_of_returns: 3,
            classification: las::point::Classification::Ground,
            scan_angle: index as f32 - 1.0,
            user_data: (index as u8).wrapping_add(7),
            point_source_id: 100_u16.wrapping_add(index as u16),
            ..Point::default()
        };
        if colored {
            point.color = Some(Color::new(0x1200, 0xAB00, 0xFF00));
        }
        writer.write_point(point).unwrap();
    }
}

fn coincident_fixture(path: &std::path::Path) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for intensity in [1, 2, 3, 4, 5] {
        writer
            .write_point(Point {
                x: 10.0,
                y: 20.0,
                z: 30.0,
                intensity,
                ..Point::default()
            })
            .unwrap();
    }
}

fn has_attribute(block: &viewer_core::PointBlock, name: &str) -> bool {
    block
        .attributes()
        .iter()
        .any(|column| column.descriptor().name() == name)
}

/// A column by name: V8 omits some columns from LOD blocks, so positions in `attributes()` vary.
fn attribute_values<'a>(block: &'a viewer_core::PointBlock, name: &str) -> &'a AttributeValues {
    block
        .attributes()
        .iter()
        .find(|column| column.descriptor().name() == name)
        .unwrap_or_else(|| panic!("block has column {name}"))
        .values()
}

fn output_pair_bytes(output: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
    (
        fs::read(output).unwrap(),
        fs::read(output_attributes_path(output)).unwrap(),
    )
}

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
            // Rotate registers: a..h = [t1+t2, a, b, c, d+t1, e, f, g].
        }
        for (slot, value) in h.iter_mut().zip(v) {
            *slot = slot.wrapping_add(value);
        }
    }
    #[expect(clippy::format_collect, reason = "test output, not a hot path")]
    let hex = h.iter().map(|word| format!("{word:08x}")).collect();
    hex
}

// --- Characterization of the spatial planner (Task 4, stage 2) -------------------------------
// These pin today's observable behaviour so the per-node-job refactor can prove it changed nothing.

/// One line per node in id order, plus the SHA-256 of `.oct` followed by its attributes file.
fn planner_snapshot(output: &std::path::Path) -> (Vec<String>, String) {
    let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(output)).unwrap())
        .unwrap();
    let hierarchy = reader.metadata().hierarchy();
    // Paged node ids are (page, index), not a dense range: number the nodes in query (preorder)
    // order and describe parents and children by that number.
    let order: Vec<_> = hierarchy.query(hierarchy.stats().root_bounds).collect();
    let number = |id: NodeId| order.iter().position(|node| node.id() == id).unwrap();
    assert_eq!(order.len(), hierarchy.stats().node_count);
    let nodes = order
        .iter()
        .enumerate()
        .map(|(id, node)| {
            let blocks = node.blocks().iter().map(|b| b.get()).collect::<Vec<_>>();
            let parent = node.parent().map(number);
            let children = hierarchy
                .child_ids(node.id())
                .iter()
                .map(|c| number(*c))
                .collect::<Vec<_>>();
            if let Some(parent) = parent {
                assert!(parent < id, "parent must precede child");
            }
            assert!(
                children.windows(2).all(|pair| pair[0] < pair[1]) && children.iter().all(|c| *c > id),
                "children ascend and follow their parent"
            );
            format!(
                "id={id} parent={parent:?} level={} points={} blocks={blocks:?} children={children:?}",
                node.level(),
                node.point_count()
            )
        })
        .collect();
    let mut bytes = fs::read(output).unwrap();
    bytes.extend(fs::read(output_attributes_path(output)).unwrap());
    (nodes, sha256_hex(&bytes))
}

fn planner_snapshot_for(
    input: &std::path::Path,
    dir: &std::path::Path,
    options: ConvertOptions,
) -> (Vec<String>, String) {
    let reference = {
        let output = dir.join("w1.oct");
        convert_file(input, &output, options.with_spatial_workers(1).unwrap()).unwrap();
        planner_snapshot(&output)
    };
    for workers in [2, 4] {
        let output = dir.join(format!("w{workers}.oct"));
        convert_file(
            input,
            &output,
            options.with_spatial_workers(workers).unwrap(),
        )
        .unwrap();
        assert_eq!(planner_snapshot(&output), reference, "workers={workers}");
    }
    reference
}

const BUDGET_HINT: &str = "budget literals depend on the estimate_memory constants and the fixture; \
    a mismatch may mean the estimate changed, not necessarily the planner";

fn eight_cluster_fixture(path: &std::path::Path) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for index in 0..800usize {
        let cluster = index % 8;
        let jitter = (index / 8) as f64 * 0.01;
        let corner = |bit: usize| if cluster & bit == 0 { 10.0 } else { 90.0 } + jitter;
        writer
            .write_point(Point {
                x: corner(1),
                y: corner(2),
                z: corner(4),
                ..Point::default()
            })
            .unwrap();
    }
}

/// Plans from an already opened reader with the native clock and executor.
fn plan_from_reader<S: viewer_convert_core::pipeline::spool_store::PlanStore>(
    reader: las::Reader,
    input: &std::path::Path,
    source: &viewer_convert_core::source::SourceInfo,
    options: ConvertOptions,
    store: &S,
    on_progress: &mut dyn FnMut(super::ConversionProgress) -> bool,
    collect: bool,
) -> Result<viewer_convert_core::spatial::SpatialPlan, ConvertError> {
    let factory = viewer_convert_core::pipeline::point_source::LasFactory::new(
        crate::las_source::FileBytes {
            path: input.to_owned(),
            len: std::fs::metadata(input).unwrap().len(),
        },
    );
    viewer_convert_core::spatial::build_spatial_plan_from_source(
        &mut viewer_convert_core::pipeline::point_source::RangeSource::Las(
            viewer_convert_core::pipeline::point_source::LasPointSource::full(reader),
        ),
        &factory,
        source,
        options,
        store,
        &crate::pipeline::SystemClock,
        &crate::executor::ThreadedExecutor,
        on_progress,
        collect,
    )
}

fn cluster_options(workers: usize) -> ConvertOptions {
    leaf_new(16, 0.01)
        .unwrap()
        .with_spatial_workers(workers)
        .unwrap()
}

fn workspace_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// Seven small corner clusters plus, in the eighth corner, a cluster of pseudo-random points
/// that splits over several levels: the root has 8 occupied children and one deep subtree.
fn deep_child_fixture(path: &std::path::Path) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    let corner = |cluster: usize, bit: usize| if cluster & bit == 0 { 10.0 } else { 90.0 };
    for cluster in 0..7usize {
        for index in 0..40usize {
            let jitter = index as f64 * 0.01;
            writer
                .write_point(Point {
                    x: corner(cluster, 1) + jitter,
                    y: corner(cluster, 2) + jitter,
                    z: corner(cluster, 4) + jitter,
                    ..Point::default()
                })
                .unwrap();
        }
    }
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as f64 / f64::from(1_u32 << 31) * 30.0
    };
    for _ in 0..3_000 {
        writer
            .write_point(Point {
                x: 70.0 + next(),
                y: 70.0 + next(),
                z: 70.0 + next(),
                ..Point::default()
            })
            .unwrap();
    }
}

fn read_pair(output: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
    (
        fs::read(output).unwrap(),
        fs::read(output_attributes_path(output)).unwrap(),
    )
}

/// Splits taken by the batches this thread planned since `split_counter` returned `before`.
/// A discarded parallel attempt (one-worker fallback) would count too; a run that reports more
/// than one worker used kept its parallel attempt, so only then do these describe the kept run.
fn split_log_since(before: usize) -> Vec<crate::executor::SplitRecord> {
    crate::executor::SPLIT_LOG.with(|log| log.borrow()[before..].to_vec())
}

fn splits_since(before: usize) -> u64 {
    split_log_since(before).len() as u64
}

fn split_counter() -> usize {
    crate::executor::SPLIT_LOG.with(|log| log.borrow().len())
}
