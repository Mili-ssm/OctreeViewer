//! Single-threaded conversion and validation over caller-supplied byte sources and sinks. The
//! block loop uses the same [`prepare_planned_block`] and [`write_prepared`] as the native
//! parallel `stream_blocks`, so both write identical bytes. Publishing the pair is the host's job.

use std::collections::HashSet;
use std::io::{Read, Seek, Write};

use viewer_core::{BlockId, DatasetSource};
use viewer_format::{FormatReader, OctreeWriter};

use crate::blocks::{ConversionPlan, output_writer, prepare_planned_block, write_prepared};
use crate::error::ConvertError;
use crate::options::ConvertOptions;
use crate::pipeline::clock::Clock;
use crate::pipeline::executor::SerialExecutor;
use crate::pipeline::hybrid::inspect_source;
use crate::pipeline::point_source::{ByteSource, LasFactory, SourceFactory};
use crate::pipeline::spool_store::PlanStore;
use crate::report::{ConversionProgress, ConversionStage, ConversionWorkerStats, emit_progress};
use crate::spatial::build_spatial_plan_from_source;

/// What validating a converted pair needs to know about its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSummary {
    pub point_count: u64,
    pub block_count: u64,
    /// Every block, in write order.
    pub block_ids: Vec<BlockId>,
    /// Blocks that hold source points (the others are LOD samples).
    pub source_block_ids: Vec<BlockId>,
    pub estimated_peak_memory_bytes: u64,
    pub omitted_dimensions: Vec<String>,
}

pub struct SerialOutput<W> {
    pub geometry: W,
    pub attributes: W,
    pub plan: PlanSummary,
}

/// Converts LAS/LAZ bytes to the split `.oct` / `.octattrs` pair on the calling thread.
/// `bytes_written` in progress reports is always 0: only the host knows what reached storage.
pub fn convert_serial<B, S, C, W>(
    input: &LasFactory<B>,
    store: &S,
    clock: &C,
    options: ConvertOptions,
    geometry: W,
    attributes: W,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
) -> Result<SerialOutput<W>, ConvertError>
where
    B: ByteSource,
    S: PlanStore,
    C: Clock,
    W: Write + Seek,
{
    emit_progress(
        on_progress,
        progress(ConversionStage::InspectingInput, 0, 0),
        0,
    )?;
    let source = inspect_source(input)?.for_options(options)?;
    let full = input
        .split(1, 1)?
        .into_iter()
        .next()
        .expect("split returns at least one range");
    let mut first = input.open(full)?;
    emit_progress(
        on_progress,
        progress(ConversionStage::PlanningMetadata, 0, source.point_count),
        0,
    )?;
    let spatial = build_spatial_plan_from_source(
        &mut first,
        input,
        &source,
        options,
        store,
        clock,
        &SerialExecutor,
        on_progress,
        false,
    )?;
    drop(first);
    let plan = ConversionPlan::from_spatial(spatial, source.frame);
    let (mut writer, encoder) = output_writer(options, &plan.metadata, geometry, attributes)?;
    emit_progress(
        on_progress,
        progress(
            ConversionStage::ReadingAndWritingBlocks,
            0,
            plan.point_count,
        ),
        0,
    )?;
    let mut processed = 0_u64;
    for block in &plan.blocks {
        let prepared = prepare_planned_block(&encoder, block, store, plan.frame)?;
        write_prepared(&mut writer, block, prepared, store)?;
        if block.is_source_points {
            processed += block.point_count() as u64;
        }
        emit_progress(
            on_progress,
            progress(
                ConversionStage::ReadingAndWritingBlocks,
                processed,
                plan.point_count,
            ),
            processed,
        )?;
    }
    if processed != plan.point_count {
        return Err(ConvertError::PointCountMismatch {
            expected: plan.point_count,
            actual: processed,
        });
    }
    let (geometry, attributes) = OctreeWriter::finish_pair(writer)?;
    Ok(SerialOutput {
        geometry,
        attributes: attributes.expect("split writer returns its companion"),
        plan: PlanSummary {
            point_count: plan.point_count,
            block_count: plan.block_count,
            block_ids: plan.blocks.iter().map(|block| block.id).collect(),
            source_block_ids: plan.source_block_ids,
            estimated_peak_memory_bytes: plan.estimated_peak_memory_bytes,
            omitted_dimensions: plan.omitted_dimensions,
        },
    })
}

/// Every block id of a written file and those of its leaf nodes (the blocks that hold source
/// points). `v7.5-pre` renumbers blocks when it writes its pages, so the plan's ids do not apply.
pub fn written_block_ids<R: Read + Seek>(
    reader: &FormatReader<R>,
) -> Result<(Vec<BlockId>, Vec<BlockId>), ConvertError> {
    let block_ids = reader
        .block_storage_stats()?
        .into_iter()
        .map(|block| block.id)
        .collect();
    let hierarchy = reader.format_metadata().dataset().hierarchy();
    let nodes = hierarchy
        .query(hierarchy.stats().root_bounds)
        .collect::<Vec<_>>();
    let parent_ids = nodes
        .iter()
        .filter_map(|node| node.parent())
        .collect::<HashSet<_>>();
    let source_block_ids = nodes
        .iter()
        .filter(|node| !parent_ids.contains(&node.id()))
        .flat_map(|node| node.blocks().iter().copied())
        .collect();
    Ok((block_ids, source_block_ids))
}

/// Rereads every block of the pair and checks that the source blocks hold `plan.point_count`
/// points: the checks of the native validation, one block at a time. `v7.5-pre` numbers its blocks
/// by page, so its ids come from the file instead of the plan.
pub fn validate_output_serial<R: Read + Seek>(
    geometry: R,
    attributes: R,
    plan: &PlanSummary,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
) -> Result<ConversionWorkerStats, ConvertError> {
    let mut reader = FormatReader::new(geometry)?;
    reader.attach_attributes(attributes)?;
    let (block_ids, source_block_ids) = written_block_ids(&reader)?;
    let source_blocks = source_block_ids.iter().copied().collect::<HashSet<_>>();
    emit_progress(
        on_progress,
        progress(ConversionStage::ValidatingOutput, 0, plan.point_count),
        0,
    )?;
    let largest_raw_block = reader
        .block_storage_stats()?
        .into_iter()
        .map(|block| block.raw_bytes.max(block.logical_bytes))
        .max()
        .unwrap_or(0);
    let worker_bytes = largest_raw_block
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(1024 * 1024))
        .ok_or(ConvertError::SizeOverflow)?;
    let mut validated_points = 0_u64;
    for block_id in block_ids {
        let points = reader.read_block(block_id)?.points().len() as u64;
        if source_blocks.contains(&block_id) {
            validated_points = validated_points
                .checked_add(points)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        emit_progress(
            on_progress,
            progress(
                ConversionStage::ValidatingOutput,
                validated_points,
                plan.point_count,
            ),
            validated_points,
        )?;
    }
    if validated_points != plan.point_count {
        return Err(ConvertError::PointCountMismatch {
            expected: plan.point_count,
            actual: validated_points,
        });
    }
    Ok(ConversionWorkerStats {
        worker_limit: 1,
        estimated_bytes_per_worker: worker_bytes,
    })
}

fn progress(stage: ConversionStage, processed: u64, total: u64) -> ConversionProgress {
    ConversionProgress {
        stage,
        points_processed: processed,
        total_points: total,
        bytes_written: 0,
    }
}
