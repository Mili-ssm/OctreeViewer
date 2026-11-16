use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use super::*;
use crate::{FormatMetadata, FormatWriterOptions, SelectedRead, SourcePointBlock};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

const ID: BlockId = BlockId::new(73);
static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "viewer-format-async-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

fn fixture() -> (FormatMetadata, SourcePointBlock) {
    let positions = vec![[0.125, 0.25, 0.5], [0.5, 0.625, 0.75], [0.875, 0.25, 0.125]];
    let colors = vec![
        ColorRgb::new(1, 2, 3),
        ColorRgb::new(40, 50, 60),
        ColorRgb::new(250, 251, 252),
    ];
    let descriptor = AttributeDescriptor::new("raw", AttributeDataType::F64, 1).unwrap();
    let values = AttributeValues::F64(vec![
        f64::from_bits(0x7ff8_0000_0000_0042),
        -0.0,
        f64::from_bits(0x3ff0_0000_0000_0001),
    ]);
    let column = AttributeColumn::new(descriptor.clone(), values, positions.len()).unwrap();
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        positions.len() as u64,
        0.0,
        vec![ID],
    )
    .unwrap();
    let hierarchy =
        SpatialHierarchy::new(vec![node], vec![descriptor], Some(positions.len() as u64)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    (
        metadata,
        SourcePointBlock::new(ID, positions, colors, vec![column]),
    )
}

fn write_fixture(dir: &TempDir, split: bool) -> (PathBuf, Option<PathBuf>) {
    write_fixture_for_version(dir, split, FormatVersion::V7_PRE1)
}

fn write_fixture_for_version(
    dir: &TempDir,
    split: bool,
    version: FormatVersion,
) -> (PathBuf, Option<PathBuf>) {
    let (metadata, block) = fixture();
    let primary = dir.path().join("fixture.oct");
    let options = FormatWriterOptions::new(0.0001, 1024 * 1024).unwrap();
    if split {
        let attributes = dir.path().join("fixture.oct.attributes");
        let mut writer = crate::FormatWriter::new_split_for_version(
            version,
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            metadata,
            options,
        )
        .unwrap();
        writer.write_block(block).unwrap();
        let (primary_bytes, attribute_bytes) = writer.finish_pair().unwrap();
        fs::write(&primary, primary_bytes.into_inner()).unwrap();
        fs::write(&attributes, attribute_bytes.unwrap().into_inner()).unwrap();
        (primary, Some(attributes))
    } else {
        let mut writer = crate::FormatWriter::new_for_version(
            version,
            Cursor::new(Vec::new()),
            metadata,
            options,
        )
        .unwrap();
        writer.write_block(block).unwrap();
        fs::write(&primary, writer.finish().unwrap().into_inner()).unwrap();
        (primary, None)
    }
}

#[test]
fn async_reader_accepts_v72_and_v73_monolithic_and_split_files() {
    runtime().block_on(async {
        for version in [FormatVersion::V7_PRE2, FormatVersion::V7_PRE3] {
            for split in [false, true] {
                let dir = TempDir::new();
                let (primary, attributes) = write_fixture_for_version(&dir, split, version);
                let reader = AsyncFormatReader::open(primary, attributes, 1, u64::MAX)
                    .await
                    .unwrap();
                assert_eq!(reader.format_version(), version);
                let read = reader.read(ID, ReadSelection::Positions).await.unwrap();
                match read.selected() {
                    SelectedRead::Positions(points) => assert_eq!(points.len(), 3),
                    other => panic!("unexpected selection result: {other:?}"),
                }
            }
        }
    });
}

async fn open(dir: &TempDir, split: bool, slots: usize, limit: u64) -> AsyncFormatReader {
    let (primary, attributes) = write_fixture(dir, split);
    AsyncFormatReader::open(primary, attributes, slots, limit)
        .await
        .unwrap()
}

#[derive(Default)]
struct Gate {
    state: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn wait(&self) {
        let mut released = self.state.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }

    fn release(&self) {
        *self.state.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

fn selections() -> [ReadSelection; 4] {
    [
        ReadSelection::Positions,
        ReadSelection::Geometry,
        ReadSelection::DisplayColors,
        ReadSelection::Attribute { schema_index: 0 },
    ]
}

#[test]
fn reads_monolithic_and_split_match_synchronous_values() {
    runtime().block_on(async {
        for split in [false, true] {
            let dir = TempDir::new();
            let reader = open(&dir, split, 2, u64::MAX).await;
            assert_eq!(reader.format_version(), FormatVersion::V7_PRE1);
            for selection in selections() {
                let result = reader
                    .read(ID, selection)
                    .await
                    .unwrap_or_else(|error| panic!("split={split} {selection:?}: {error}"));
                #[expect(
                    clippy::match_same_arms,
                    reason = "each arm binds a different payload type"
                )]
                match (selection, result.selected()) {
                    (ReadSelection::Positions, SelectedRead::Positions(points)) => {
                        assert_eq!(points.len(), 3);
                    }
                    (ReadSelection::Geometry, SelectedRead::Geometry(points)) => {
                        assert_eq!(points.len(), 3);
                    }
                    (ReadSelection::DisplayColors, SelectedRead::DisplayColors(colors)) => {
                        assert_eq!(
                            colors,
                            &[
                                ColorRgb::new(1, 2, 3),
                                ColorRgb::new(40, 50, 60),
                                ColorRgb::new(250, 251, 252)
                            ]
                        );
                    }
                    (ReadSelection::Attribute { .. }, SelectedRead::Attribute(column)) => {
                        match column.values() {
                            AttributeValues::F64(values) => assert_eq!(
                                values
                                    .iter()
                                    .map(|value| value.to_bits())
                                    .collect::<Vec<_>>(),
                                vec![
                                    0x7ff8_0000_0000_0042,
                                    (-0.0f64).to_bits(),
                                    0x3ff0_0000_0000_0001
                                ]
                            ),
                            other => panic!("unexpected attribute values: {other:?}"),
                        }
                    }
                    pair => panic!("selection/result mismatch: {pair:?}"),
                }
                drop(result);
            }
            assert_eq!(reader.reserved_bytes(), 0);
        }
    });
}

#[test]
fn cloned_reader_runs_two_gated_workers_and_releases_after_last_arc() {
    runtime().block_on(async {
        let dir = TempDir::new();
        let reader = open(&dir, false, 2, u64::MAX).await;
        let clone = reader.clone();
        let gate = Arc::new(Gate::default());
        let (tx1, rx1) = tokio::sync::oneshot::channel();
        let (tx2, rx2) = tokio::sync::oneshot::channel();
        let mut notices = [Some(tx1), Some(tx2)];
        let mut receivers = [rx1, rx2];
        let mut tasks = Vec::new();
        for (which, current) in [reader.clone(), clone].into_iter().enumerate() {
            let gate = Arc::clone(&gate);
            let tx = notices[which].take().unwrap();
            tasks.push(tokio::spawn(async move {
                current
                    .read_with(ID, ReadSelection::Positions, move |reader, reservation| {
                        let _ = tx.send(());
                        gate.wait();
                        reader.read_reserved(ID, ReadSelection::Positions, reservation)
                    })
                    .await
            }));
        }
        let first = tokio::time::timeout(Duration::from_secs(3), &mut receivers[0]).await;
        let second = tokio::time::timeout(Duration::from_secs(3), &mut receivers[1]).await;
        gate.release();
        assert!(first.is_ok() && second.is_ok(), "workers failed to overlap");
        for task in tasks {
            assert!(
                tokio::time::timeout(Duration::from_secs(3), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_ok()
            );
        }
        assert_eq!(reader.reserved_bytes(), 0);
    });
}

#[test]
fn cancellation_of_active_worker_keeps_slot_and_quota_until_worker_returns() {
    runtime().block_on(async {
        let dir = TempDir::new();
        let reader = open(&dir, false, 1, u64::MAX).await;
        let selection = ReadSelection::Positions;
        let peak = reader
            .plan_read(ID, selection)
            .unwrap()
            .memory()
            .peak_bytes();
        let gate = Arc::new(Gate::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let worker_reader = reader.clone();
        let worker_gate = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            worker_reader
                .read_with(ID, selection, move |reader, reservation| {
                    let _ = started_tx.send(());
                    worker_gate.wait();
                    reader.read_reserved(ID, selection, reservation)
                })
                .await
        });
        let started = tokio::time::timeout(Duration::from_secs(3), started_rx).await;
        task.abort();
        let _ = task.await;
        let retained = reader.reserved_bytes();
        let waiting_reader = reader.clone();
        let (waiting_tx, waiting_rx) = tokio::sync::oneshot::channel();
        let waiting = tokio::spawn(async move {
            waiting_reader
                .read_with(ID, selection, move |reader, reservation| {
                    let _ = waiting_tx.send(());
                    reader.read_reserved(ID, selection, reservation)
                })
                .await
        });
        // The second closure can start only after the cancelled caller's blocking worker exits.
        let was_blocked = tokio::time::timeout(Duration::from_millis(100), waiting_rx)
            .await
            .is_err();
        gate.release();
        let resumed = tokio::time::timeout(Duration::from_secs(3), waiting).await;
        assert!(started.is_ok(), "worker did not start before cancellation");
        assert!(
            retained >= peak,
            "worker cancellation released reservation early: {retained} < {peak}"
        );
        assert!(
            was_blocked,
            "second worker did not remain behind the occupied slot"
        );
        assert!(resumed.unwrap().unwrap().is_ok());
        assert_eq!(reader.reserved_bytes(), 0);
    });
}

#[test]
fn quota_waiter_is_woken_when_last_budgeted_arc_drops() {
    runtime().block_on(async {
        let dir = TempDir::new();
        let provisional = open(&dir, false, 2, u64::MAX).await;
        let selection = ReadSelection::Attribute { schema_index: 0 };
        let plan = provisional.plan_read(ID, selection).unwrap().memory();
        let output = plan.output_bytes();
        let limit = output + plan.peak_bytes() - 1;
        let reader = open(&dir, false, 2, limit).await;
        let held = reader.read(ID, selection).await.unwrap();
        assert_eq!(reader.reserved_bytes(), held.reserved_bytes());
        let held_clone = Arc::clone(&held);
        drop(held);
        assert_eq!(reader.reserved_bytes(), held_clone.reserved_bytes());
        let waiting_reader = reader.clone();
        let mut waiter = tokio::spawn(async move { waiting_reader.read(ID, selection).await });
        let stayed_pending = tokio::time::timeout(Duration::from_millis(100), &mut waiter)
            .await
            .is_err();
        drop(held_clone);
        let result = tokio::time::timeout(Duration::from_secs(3), waiter).await;
        assert!(stayed_pending);
        let output = result.unwrap().unwrap().unwrap();
        assert!(reader.reserved_bytes() > 0);
        drop(output);
        assert_eq!(reader.reserved_bytes(), 0);
    });
}
