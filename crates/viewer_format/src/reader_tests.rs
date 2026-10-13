use std::{
    cell::RefCell,
    io::{Cursor, Read, Seek, SeekFrom},
    rc::Rc,
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
    BlockRequestScheduler, ColorRgb, CoordinateFrame, DatasetMetadata, DatasetSource, FrustumPlane,
    HierarchySemantics, LodSelectionConfig, NodeId, RequestCompletion, SpatialBounds,
    SpatialFrustum, SpatialHierarchy, SpatialNode,
};

use crate::{FormatMetadata, FormatReaderLimits, SourcePointBlock};

use super::{FormatError, FormatReader};
use crate::writer::{FormatWriter, FormatWriterOptions};

fn metadata() -> FormatMetadata {
    let origin = [1_000_000_000_000.0, -2_000_000_000_000.0, 10.0];
    let bounds =
        SpatialBounds::new(origin, [origin[0] + 2.0, origin[1] + 1.0, origin[2] + 1.0]).unwrap();
    let block_id = BlockId::new(42);
    let node = SpatialNode::new(NodeId::new(0), None, 0, bounds, 2, 0.25, vec![block_id]).unwrap();
    let schema = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], vec![schema], Some(2)).unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
        Some("EPSG:4978".to_owned()),
        Some("uniform voxel sampling".to_owned()),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn shared_dataset_metadata_reuses_the_same_hierarchy_allocation() {
    let metadata = metadata();
    let first = metadata.shared_dataset();
    let second = metadata.shared_dataset();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

fn source_block() -> SourcePointBlock {
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let column = AttributeColumn::new(descriptor, AttributeValues::U16(vec![100, 200]), 2).unwrap();
    SourcePointBlock::new(
        BlockId::new(42),
        vec![[0.25, 0.5, 0.0], [1.5, 0.75, 1.0]],
        vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
        vec![column],
    )
}

fn additive_metadata() -> FormatMetadata {
    let base = metadata();
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(0),
                None,
                0,
                base.dataset().hierarchy().stats().root_bounds,
                2,
                0.25,
                vec![BlockId::new(42)],
            )
            .unwrap(),
        ],
        base.dataset().hierarchy().attributes().to_vec(),
        Some(2),
        HierarchySemantics::Additive,
    )
    .unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(base.dataset().coordinate_frame(), hierarchy),
        base.crs().map(str::to_owned),
        base.sampling_policy().map(str::to_owned),
        Vec::new(),
    )
    .unwrap()
}

fn format_bytes() -> Vec<u8> {
    let options = FormatWriterOptions::new(0.001, 1024).unwrap();
    let mut writer = FormatWriter::new(Cursor::new(Vec::new()), metadata(), options).unwrap();
    writer.write_block(source_block()).unwrap();
    writer.finish().unwrap().into_inner()
}

fn empty_attribute_range_bytes() -> Vec<u8> {
    let origin = [0.0; 3];
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let hierarchy = SpatialHierarchy::new(
        vec![
            SpatialNode::new(
                NodeId::new(0),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                0,
                0.0,
                vec![],
            )
            .unwrap(),
        ],
        vec![descriptor],
        Some(0),
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.001, 1024).unwrap(),
    )
    .unwrap()
    .finish()
    .unwrap()
    .into_inner()
}

fn multicomponent_range_bytes() -> Vec<u8> {
    const COMPONENTS: usize = 65;
    let origin = [0.0; 3];
    let block_id = BlockId::new(42);
    let descriptor =
        AttributeDescriptor::new("vector", AttributeDataType::U64, COMPONENTS as u32).unwrap();
    let hierarchy = SpatialHierarchy::new(
        vec![
            SpatialNode::new(
                NodeId::new(0),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                2,
                0.0,
                vec![block_id],
            )
            .unwrap(),
        ],
        vec![descriptor.clone()],
        Some(2),
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let mut values = (0..COMPONENTS)
        .map(|component| component as u64)
        .chain((0..COMPONENTS).map(|component| component as u64 + 10))
        .collect::<Vec<_>>();
    values[0] = u64::MAX - 1;
    values[COMPONENTS] = u64::MAX;
    values[1] = 7;
    values[COMPONENTS + 1] = 11;
    let column = AttributeColumn::new(descriptor, AttributeValues::U64(values), 2).unwrap();
    let block = SourcePointBlock::new(
        block_id,
        vec![[0.0; 3]; 2],
        vec![ColorRgb::new(1, 2, 3); 2],
        vec![column],
    );
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.001, 4096).unwrap(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    writer.finish().unwrap().into_inner()
}

fn additive_format_bytes() -> Vec<u8> {
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        additive_metadata(),
        FormatWriterOptions::new(0.01, 1024 * 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(source_block()).unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn reads_metadata_and_only_the_requested_point_block() {
    let bytes = format_bytes();
    let reads = Rc::new(RefCell::new(Vec::new()));
    let reader = ReadSpy {
        inner: Cursor::new(bytes.clone()),
        reads: Rc::clone(&reads),
    };
    let mut format = FormatReader::new(reader).unwrap();
    let storage = format.block_storage_stats().unwrap();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].id, BlockId::new(42));
    assert_eq!(storage[0].point_count, 2);
    assert_eq!(storage[0].raw_bytes, 34);
    assert!(storage[0].stored_bytes < storage[0].raw_bytes);
    let index_offset = u64::from_le_bytes(
        bytes[bytes.len() - 32..bytes.len() - 24]
            .try_into()
            .unwrap(),
    );
    assert!(
        reads
            .borrow()
            .iter()
            .all(|(start, length)| *start + *length as u64 <= 24 || *start >= index_offset)
    );
    assert_eq!(format.format_metadata().crs(), Some("EPSG:4978"));
    assert_eq!(format.format_metadata().visual_scale(), 1.0);
    assert_eq!(format.metadata().coordinate_frame().origin()[0], 1e12);
    assert_eq!(
        format.metadata().hierarchy().semantics(),
        HierarchySemantics::Replacement
    );

    let entry = format.blocks[&BlockId::new(42)].clone();
    reads.borrow_mut().clear();
    assert_eq!(format.attribute_range(0, 0).unwrap(), Some([100.0, 200.0]));
    assert_eq!(format.attribute_range(0, 0).unwrap(), Some([100.0, 200.0]));
    assert!(reads.borrow().is_empty());
    let block = format.read_block(BlockId::new(42)).unwrap();
    assert_eq!(block.points().points()[0].position, [0.25, 0.5, 0.0]);
    assert_eq!(block.points().points()[1].color, ColorRgb::new(40, 50, 60));
    assert_eq!(
        block.attributes()[0].values(),
        &AttributeValues::U16(vec![100, 200])
    );
    assert!(reads.borrow().iter().all(|(start, length)| {
        *start >= entry.offset && *start + *length as u64 <= entry.offset + entry.payload_length
    }));
}

#[test]
fn forked_readers_share_the_index_and_read_with_independent_cursors() {
    let bytes = format_bytes();
    let reader = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([100.0, 200.0]));
    let first = reader.fork_reader(Cursor::new(bytes.clone()));
    let second = reader.fork_reader(Cursor::new(bytes));
    assert!(std::sync::Arc::ptr_eq(&first.blocks, &second.blocks));
    assert!(std::sync::Arc::ptr_eq(&first.metadata, &second.metadata));
    assert!(std::sync::Arc::ptr_eq(
        &first.attribute_range_cache,
        &second.attribute_range_cache
    ));
    assert_eq!(first.attribute_range(0, 0).unwrap(), Some([100.0, 200.0]));

    let (first_points, second_points) = std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            let mut reader = first;
            reader.read_block(BlockId::new(42)).unwrap().points().len()
        });
        let second = scope.spawn(move || {
            let mut reader = second;
            reader.read_block(BlockId::new(42)).unwrap().points().len()
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!(first_points, 2);
    assert_eq!(second_points, 2);
}

#[test]
fn scheduler_tickets_read_selected_format_blocks_through_dataset_source() {
    let mut reader = FormatReader::new(Cursor::new(format_bytes())).unwrap();
    let bounds = reader.metadata().hierarchy().stats().root_bounds;
    let min = bounds.min();
    let max = bounds.max();
    let frustum = SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], -min[0]).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], max[0]).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], -min[1]).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], max[1]).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], -min[2]).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], max[2]).unwrap(),
    ])
    .unwrap();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    scheduler
        .update_view(
            reader.metadata().hierarchy(),
            &frustum,
            LodSelectionConfig::new(1.0, 2).unwrap(),
            min,
            |_| 0.0,
        )
        .unwrap();

    let request = scheduler.take_next().unwrap().unwrap();
    let block = reader.read_block(request.ticket().block_id()).unwrap();
    assert_eq!(block.points().len(), 2);
    assert_eq!(
        scheduler
            .complete(request.ticket(), Ok(block.id()))
            .unwrap(),
        RequestCompletion::Delivered
    );
    assert_eq!(scheduler.pending_count(), 0);
    assert_eq!(scheduler.in_flight_count(), 0);
}

#[test]
fn verifies_block_crc_when_read_and_keeps_open_lazy() {
    let mut bytes = format_bytes();
    let format = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    let entry = format.blocks[&BlockId::new(42)].clone();
    bytes[entry.offset as usize] ^= 1;
    let mut corrupted = FormatReader::new(Cursor::new(bytes)).unwrap();

    assert!(matches!(
        corrupted.read_block(BlockId::new(42)),
        Err(FormatError::InvalidBlockChecksum(id)) if id.get() == 42
    ));
}

#[test]
fn applies_index_limit_before_reading_the_index() {
    let bytes = format_bytes();
    let limits = FormatReaderLimits::new(1, 1024, 10, 10, 10, 10, 100).unwrap();

    assert!(matches!(
        FormatReader::with_limits(Cursor::new(bytes), limits),
        Err(FormatError::IndexExceedsLimit { limit: 1, .. })
    ));
}

#[test]
fn rejects_corrupt_index_and_blocks_over_the_configured_limit() {
    let mut bytes = format_bytes();
    let index_offset = u64::from_le_bytes(
        bytes[bytes.len() - 32..bytes.len() - 24]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[index_offset] ^= 1;
    assert!(matches!(
        FormatReader::new(Cursor::new(bytes)),
        Err(FormatError::InvalidChecksum("index"))
    ));

    let limits = FormatReaderLimits::new(1024, 1, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        FormatReader::with_limits(Cursor::new(format_bytes()), limits),
        Err(FormatError::BlockExceedsLimit { limit: 1, .. })
    ));
}

#[test]
fn rejects_truncated_and_non_v5_version_files() {
    let mut truncated = format_bytes();
    truncated.pop();
    assert!(matches!(
        FormatReader::new(Cursor::new(truncated)),
        Err(FormatError::InvalidFooter)
    ));

    for version in [1_u16, 2, 3, 4, 6] {
        let mut unsupported = format_bytes();
        unsupported[8..10].copy_from_slice(&version.to_le_bytes());
        let crc = crate::crc32(&unsupported[..20]);
        unsupported[20..24].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            FormatReader::new(Cursor::new(unsupported)),
            Err(FormatError::UnsupportedVersion(actual)) if actual == version
        ));
    }
    let mut bad_footer = format_bytes();
    let footer = bad_footer.len() - 40;
    bad_footer[footer..footer + 8].copy_from_slice(b"OCTVEND3");
    let checksum = crate::crc32(&bad_footer[footer..footer + 36]);
    bad_footer[footer + 36..].copy_from_slice(&checksum.to_le_bytes());
    assert!(matches!(
        FormatReader::new(Cursor::new(bad_footer)),
        Err(FormatError::InvalidFooter)
    ));
}

#[test]
fn reads_v5_visual_scale_and_additive_hierarchy() {
    let metadata = additive_metadata().with_visual_scale(50.0).unwrap();
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.001, 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(source_block()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 5);
    assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND5");
    let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.format_metadata().visual_scale(), 50.0);
    assert_eq!(
        reader.metadata().hierarchy().semantics(),
        HierarchySemantics::Additive
    );
    assert_eq!(reader.format_metadata().crs(), Some("EPSG:4978"));
    assert_eq!(
        reader
            .read_block(BlockId::new(42))
            .unwrap()
            .points()
            .points()[0]
            .position,
        [0.25, 0.5, 0.0]
    );
}

#[test]
fn reads_additive_v5_and_preserves_ancestor_payload_counts() {
    let bytes = additive_format_bytes();
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 5);
    let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(
        reader.metadata().hierarchy().semantics(),
        HierarchySemantics::Additive
    );
    assert_eq!(reader.format_metadata().visual_scale(), 1.0);
    assert_eq!(
        reader
            .metadata()
            .hierarchy()
            .node(NodeId::new(0))
            .unwrap()
            .point_count(),
        2
    );
    assert_eq!(
        reader
            .read_point_block(BlockId::new(42))
            .unwrap()
            .points()
            .len(),
        2
    );
}

#[test]
fn reads_an_independently_compressed_block() {
    let origin = [0.0; 3];
    let block_id = BlockId::new(9);
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node =
        SpatialNode::new(NodeId::new(0), None, 0, bounds, 4096, 0.0, vec![block_id]).unwrap();
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let hierarchy =
        SpatialHierarchy::new(vec![node], vec![descriptor.clone()], Some(4096)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let column =
        AttributeColumn::new(descriptor, AttributeValues::U16(vec![128; 4096]), 4096).unwrap();
    let source = SourcePointBlock::new(
        block_id,
        vec![[0.25, 0.5, 0.75]; 4096],
        vec![ColorRgb::new(20, 40, 60); 4096],
        vec![column],
    );
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.0, 1024 * 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(source).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();

    assert!(
        reader.blocks[&block_id]
            .payloads
            .iter()
            .any(|payload| payload.codec == crate::BlockCodec::Lz4)
    );
    let storage = reader.block_storage_stats().unwrap();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].point_count, 4096);
    assert_eq!(storage[0].raw_bytes, 4096 * 17);
    assert!(storage[0].stored_bytes < storage[0].raw_bytes);
    let block = reader.read_block(block_id).unwrap();
    assert_eq!(block.points().len(), 4096);
    assert_eq!(block.points().points()[2048].position, [0.25, 0.5, 0.75]);
    assert_eq!(
        block.attributes()[0].values(),
        &AttributeValues::U16(vec![128; 4096])
    );
}

#[test]
fn round_trips_a_larger_synthetic_block() {
    let block_id = BlockId::new(77);
    let point_count = 65_536;
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        point_count,
        0.0,
        vec![block_id],
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], Vec::new(), Some(point_count)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let source = SourcePointBlock::new(
        block_id,
        vec![[0.25, 0.5, 0.75]; point_count as usize],
        vec![ColorRgb::new(20, 40, 60); point_count as usize],
        Vec::new(),
    );
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.0, 4 * 1024 * 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(source).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();

    let block = reader.read_block(block_id).unwrap();
    assert_eq!(block.points().len(), point_count as usize);
    assert_eq!(block.points().points()[0].position, [0.25, 0.5, 0.75]);
    assert_eq!(
        block.points().points()[point_count as usize - 1].color,
        ColorRgb::new(20, 40, 60)
    );
}

#[test]
fn preserves_each_supported_attribute_type_and_float_bits() {
    let block_id = BlockId::new(100);
    let descriptors = vec![
        AttributeDescriptor::new("u8", AttributeDataType::U8, 2).unwrap(),
        AttributeDescriptor::new("i8", AttributeDataType::I8, 1).unwrap(),
        AttributeDescriptor::new("u16", AttributeDataType::U16, 1).unwrap(),
        AttributeDescriptor::new("i16", AttributeDataType::I16, 1).unwrap(),
        AttributeDescriptor::new("u32", AttributeDataType::U32, 1).unwrap(),
        AttributeDescriptor::new("i32", AttributeDataType::I32, 1).unwrap(),
        AttributeDescriptor::new("u64", AttributeDataType::U64, 1).unwrap(),
        AttributeDescriptor::new("i64", AttributeDataType::I64, 1).unwrap(),
        AttributeDescriptor::new("f32", AttributeDataType::F32, 1).unwrap(),
        AttributeDescriptor::new("f64", AttributeDataType::F64, 1).unwrap(),
    ];
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node = SpatialNode::new(NodeId::new(0), None, 0, bounds, 2, 0.0, vec![block_id]).unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], descriptors.clone(), Some(2)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let source_values = vec![
        AttributeValues::U8(vec![1, 2, 3, 4]),
        AttributeValues::I8(vec![-1, 2]),
        AttributeValues::U16(vec![500, u16::MAX]),
        AttributeValues::I16(vec![-300, 22]),
        AttributeValues::U32(vec![u32::MAX, 1]),
        AttributeValues::I32(vec![-22, 99]),
        AttributeValues::U64(vec![u64::MAX, 42]),
        AttributeValues::I64(vec![-100, 999]),
        AttributeValues::F32(vec![f32::from_bits(0x7FC0_1234), 1.5]),
        AttributeValues::F64(vec![f64::from_bits(0x7FF8_0000_0000_1234), -0.0]),
    ];
    let attributes = descriptors
        .into_iter()
        .zip(source_values.clone())
        .map(|(descriptor, values)| AttributeColumn::new(descriptor, values, 2).unwrap())
        .collect();
    let source = SourcePointBlock::new(
        block_id,
        vec![[0.0; 3], [1.0; 3]],
        vec![ColorRgb::new(0, 1, 2), ColorRgb::new(255, 254, 253)],
        attributes,
    );
    {
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata.clone(),
            FormatWriterOptions::new(0.0, 1024).unwrap(),
        )
        .unwrap();
        writer
            .write_block(SourcePointBlock::new(
                source.id(),
                source.positions_local().to_vec(),
                source.colors().to_vec(),
                source.attributes().to_vec(),
            ))
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
        let block = reader.read_block(block_id).unwrap();

        for (column, expected) in block.attributes()[..8].iter().zip(&source_values[..8]) {
            assert_eq!(column.values(), expected);
        }
        let AttributeValues::F32(f32_values) = block.attributes()[8].values() else {
            panic!("expected f32 values");
        };
        assert_eq!(f32_values[0].to_bits(), 0x7FC0_1234);
        assert_eq!(f32_values[1].to_bits(), 1.5_f32.to_bits());
        let AttributeValues::F64(f64_values) = block.attributes()[9].values() else {
            panic!("expected f64 values");
        };
        assert_eq!(f64_values[0].to_bits(), 0x7FF8_0000_0000_1234);
        assert_eq!(f64_values[1].to_bits(), (-0.0_f64).to_bits());
    }
}

fn columnar_format_bytes() -> Vec<u8> {
    let base = metadata();
    let ids = vec![BlockId::new(42), BlockId::new(43)];
    let vector = AttributeDescriptor::new("vector", AttributeDataType::F32, 2).unwrap();
    let mut schema = base.dataset().hierarchy().attributes().to_vec();
    schema.push(vector.clone());
    let root = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        base.dataset().hierarchy().stats().root_bounds,
        4,
        0.25,
        ids.clone(),
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![root],
        schema,
        Some(4),
        HierarchySemantics::Additive,
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(base.dataset().coordinate_frame(), hierarchy),
        base.crs().map(str::to_owned),
        None,
        Vec::new(),
    )
    .unwrap()
    .with_visual_scale(50.0)
    .unwrap();
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.001, 1024).unwrap(),
    )
    .unwrap();
    for id in ids {
        let block = source_block();
        let mut attributes = block.attributes().to_vec();
        attributes.push(
            AttributeColumn::new(
                vector.clone(),
                AttributeValues::F32(vec![0.5, -1.0, 1.5, -2.0]),
                2,
            )
            .unwrap(),
        );
        writer
            .write_block(SourcePointBlock::new(
                id,
                block.positions_local().to_vec(),
                block.colors().to_vec(),
                attributes,
            ))
            .unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn v5_reads_only_geometry_or_one_requested_column_and_keeps_row_identity() {
    let bytes = columnar_format_bytes();
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 5);
    assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND5");
    let reads = Rc::new(RefCell::new(Vec::new()));
    let mut reader = FormatReader::new(ReadSpy {
        inner: Cursor::new(bytes),
        reads: Rc::clone(&reads),
    })
    .unwrap();
    assert_eq!(reader.format_metadata().visual_scale(), 50.0);
    assert_eq!(
        reader.metadata().hierarchy().semantics(),
        HierarchySemantics::Additive
    );
    let entry = reader.blocks[&BlockId::new(42)].clone();
    let payloads = &entry.payloads;
    let index_offset = reader
        .blocks
        .values()
        .map(|entry| entry.offset + entry.payload_length)
        .max()
        .unwrap();
    assert!(
        reads
            .borrow()
            .iter()
            .all(|(start, length)| *start + *length as u64 <= 24 || *start >= index_offset)
    );

    let reader: &mut dyn crate::OctreeReader<Error = FormatError> = &mut reader;
    reads.borrow_mut().clear();
    let geometry = reader.read_positions(BlockId::new(42)).unwrap();
    assert_eq!(geometry.points()[0].position, [0.25, 0.5, 0.0]);
    assert_eq!(geometry.points()[1].color, ColorRgb::new(160, 160, 160));
    let assert_only_column = |column: &crate::layout::PayloadDirectoryEntry| {
        let reads = reads.borrow();
        assert_eq!(
            reads.iter().map(|(_, length)| *length as u64).sum::<u64>(),
            column.stored_length
        );
        assert!(
            reads
                .iter()
                .all(|(start, length)| *start >= entry.offset + column.offset
                    && *start + *length as u64
                        <= entry.offset + column.offset + column.stored_length)
        );
    };
    assert_only_column(&payloads[0]);

    reads.borrow_mut().clear();
    assert_eq!(
        reader.read_attribute_row(BlockId::new(42), 1, 1).unwrap(),
        AttributeValues::F32(vec![1.5, -2.0])
    );
    assert_only_column(&payloads[3]);
    reads.borrow_mut().clear();
    assert_eq!(
        reader.read_attribute(BlockId::new(42), 0).unwrap().values(),
        &AttributeValues::U16(vec![100, 200])
    );
    assert_only_column(&payloads[2]);

    for id in [BlockId::new(42), BlockId::new(43)] {
        let block = reader.read_block(id).unwrap();
        assert_eq!(
            block.points().points()[0].position,
            geometry.points()[0].position
        );
        assert_eq!(block.points().points()[1].color, ColorRgb::new(40, 50, 60));
        assert_eq!(block.attributes().len(), 2);
        assert_eq!(
            block.attributes()[0].values(),
            &AttributeValues::U16(vec![100, 200])
        );
    }
    reads.borrow_mut().clear();
    assert!(matches!(
        reader.read_attribute_row(BlockId::new(42), 2, 0),
        Err(FormatError::UnknownRow { .. })
    ));
    assert!(matches!(
        reader.read_attribute(BlockId::new(42), 2),
        Err(FormatError::UnknownAttribute(2))
    ));
    assert!(matches!(
        reader.read_geometry(BlockId::new(99)),
        Err(FormatError::UnknownBlock(_))
    ));
    assert!(reads.borrow().is_empty());
}

#[test]
fn payload_stats_report_v5_columns_without_io() {
    let bytes = columnar_format_bytes();
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 5);
    let reads = Rc::new(RefCell::new(Vec::new()));
    let reader = FormatReader::new(ReadSpy {
        inner: Cursor::new(bytes),
        reads: Rc::clone(&reads),
    })
    .unwrap();
    reads.borrow_mut().clear();

    let aggregates = reader.block_storage_stats().unwrap();
    let payloads = reader.block_payload_storage_stats().unwrap();

    assert!(reads.borrow().is_empty());
    assert_eq!(aggregates.len(), 2);
    assert_eq!(payloads.len(), 8);
    assert_eq!(payloads[0].kind, super::BlockPayloadKind::Geometry);
    assert_eq!(
        payloads[2].kind,
        super::BlockPayloadKind::Attribute {
            schema_index: 0,
            name: "intensity".to_owned()
        }
    );
    assert_eq!(
        payloads[3].kind,
        super::BlockPayloadKind::Attribute {
            schema_index: 1,
            name: "vector".to_owned()
        }
    );
    for aggregate in &aggregates {
        let matching = payloads.iter().filter(|payload| payload.id == aggregate.id);
        assert_eq!(
            matching
                .clone()
                .map(|payload| payload.stored_bytes)
                .sum::<u64>(),
            aggregate.stored_bytes
        );
        assert_eq!(
            matching.map(|payload| payload.raw_bytes).sum::<u64>(),
            aggregate.raw_bytes
        );
        assert_eq!(
            aggregate.stored_bytes,
            reader.blocks[&aggregate.id]
                .payloads
                .iter()
                .map(|payload| payload.stored_length)
                .sum()
        );
    }
}

#[test]
fn v5_checksums_are_lazy_and_independent_for_geometry_and_each_attribute() {
    let bytes = columnar_format_bytes();
    let reader = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    let entry = &reader.blocks[&BlockId::new(42)];
    for column_index in 0..4 {
        let mut corrupted = bytes.clone();
        corrupted[(entry.offset + entry.payloads[column_index].offset) as usize] ^= 1;
        let mut reader = FormatReader::new(Cursor::new(corrupted)).unwrap();
        let id = BlockId::new(42);
        for index in 0..4 {
            let result = match index {
                0 => reader.read_positions(id).map(|_| ()),
                1 => reader.read_display_colors(id).map(|_| ()),
                _ => reader.read_attribute(id, index - 2).map(|_| ()),
            };
            if index as usize == column_index {
                assert!(matches!(result, Err(FormatError::InvalidBlockChecksum(_))));
            } else {
                result.unwrap();
            }
        }
        assert!(matches!(
            reader.read_block(id),
            Err(FormatError::InvalidBlockChecksum(_))
        ));
    }
}

fn repair_index_and_footer_checksums(bytes: &mut [u8]) {
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let checksum = crate::crc32(&bytes[index..footer]);
    bytes[footer + 32..footer + 36].copy_from_slice(&checksum.to_le_bytes());
    let checksum = crate::crc32(&bytes[footer..footer + 36]);
    bytes[footer + 36..].copy_from_slice(&checksum.to_le_bytes());
}

#[test]
fn v5_rejects_invalid_payload_directory_before_payload_reads() {
    let bytes = columnar_format_bytes();
    let footer = bytes.len() - 40;
    // Two block entries, each containing a 93-byte common entry and three 29-byte payload entries.
    let first_column = footer - 2 * (93 + 4 * 29) + 93;
    for (offset, replacement) in [
        (first_column - 5, vec![1]), // reserved aggregate codec
        (first_column - 4, 1_u32.to_le_bytes().to_vec()), // reserved aggregate checksum
        (first_column, 1_u64.to_le_bytes().to_vec()), // gap at geometry start
        (first_column + 8, u64::MAX.to_le_bytes().to_vec()), // stored size overflow
        (first_column + 16, u64::MAX.to_le_bytes().to_vec()), // decoded length mismatch
        (first_column + 24, vec![255]), // unsupported codec
        (first_column + 29, 0_u64.to_le_bytes().to_vec()), // attribute overlaps geometry
    ] {
        let mut corrupted = bytes.clone();
        corrupted[offset..offset + replacement.len()].copy_from_slice(&replacement);
        repair_index_and_footer_checksums(&mut corrupted);
        let reads = Rc::new(RefCell::new(Vec::new()));
        let index = u64::from_le_bytes(corrupted[footer + 8..footer + 16].try_into().unwrap());
        assert!(
            FormatReader::new(ReadSpy {
                inner: Cursor::new(corrupted),
                reads: Rc::clone(&reads)
            })
            .is_err()
        );
        assert!(
            reads
                .borrow()
                .iter()
                .all(|(start, length)| *start + *length as u64 <= 24 || *start >= index)
        );
    }
    let limits = FormatReaderLimits::new(4096, 49, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        FormatReader::with_limits(Cursor::new(bytes), limits),
        Err(FormatError::BlockExceedsLimit { .. })
    ));
}

#[test]
fn v5_round_trips_empty_geometry_and_attribute_columns() {
    let id = BlockId::new(0);
    let descriptor = AttributeDescriptor::new("empty", AttributeDataType::U64, 2).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
        0,
        0.0,
        vec![id],
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], vec![descriptor.clone()], Some(0)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.0, 1024).unwrap(),
    )
    .unwrap();
    writer
        .write_block(SourcePointBlock::new(
            id,
            Vec::new(),
            Vec::new(),
            vec![AttributeColumn::new(descriptor, AttributeValues::U64(Vec::new()), 0).unwrap()],
        ))
        .unwrap();
    let mut reader = FormatReader::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap();
    assert_eq!(reader.read_geometry(id).unwrap().len(), 0);
    assert_eq!(
        reader.read_attribute(id, 0).unwrap().values(),
        &AttributeValues::U64(Vec::new())
    );
    assert_eq!(reader.read_block(id).unwrap().points().len(), 0);
    assert!(matches!(
        reader.read_attribute_row(id, 0, 0),
        Err(FormatError::UnknownRow { .. })
    ));
}

#[derive(Clone)]
struct ReadSpy {
    inner: Cursor<Vec<u8>>,
    reads: Rc<RefCell<Vec<(u64, usize)>>>,
}

impl Read for ReadSpy {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let start = self.inner.position();
        let read = self.inner.read(buffer)?;
        if read > 0 {
            self.reads.borrow_mut().push((start, read));
        }
        Ok(read)
    }
}

impl Seek for ReadSpy {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

fn split_bytes(block: SourcePointBlock) -> (Vec<u8>, Vec<u8>) {
    let mut writer = FormatWriter::new_split(
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata(),
        FormatWriterOptions::new(0.001, 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

#[test]
fn split_geometry_is_gray_and_selective_reads_do_not_touch_other_file() {
    let (primary, attributes) = split_bytes(source_block());
    let primary_reads = Rc::new(RefCell::new(Vec::new()));
    let attribute_reads = Rc::new(RefCell::new(Vec::new()));
    let mut reader = FormatReader::new(ReadSpy {
        inner: Cursor::new(primary),
        reads: primary_reads.clone(),
    })
    .unwrap();
    let id = BlockId::new(42);
    assert!(!reader.has_attributes());
    let gray = reader.read_block(id).unwrap();
    assert!(gray.attributes().is_empty());
    assert!(
        gray.points()
            .points()
            .iter()
            .all(|p| p.color == ColorRgb::new(160, 160, 160))
    );
    assert!(matches!(
        reader.read_attribute(id, 0),
        Err(FormatError::AttributesUnavailable)
    ));
    reader
        .attach_attributes(ReadSpy {
            inner: Cursor::new(attributes),
            reads: attribute_reads.clone(),
        })
        .unwrap();
    primary_reads.borrow_mut().clear();
    attribute_reads.borrow_mut().clear();
    reader.read_positions(id).unwrap();
    assert!(attribute_reads.borrow().is_empty());
    assert_eq!(
        primary_reads
            .borrow()
            .iter()
            .map(|(_, n)| *n as u64)
            .sum::<u64>(),
        reader.blocks[&id].payloads[0].stored_length
    );
    primary_reads.borrow_mut().clear();
    assert_eq!(
        reader.read_attribute(id, 0).unwrap().values(),
        &AttributeValues::U16(vec![100, 200])
    );
    assert!(primary_reads.borrow().is_empty());
    assert_eq!(
        attribute_reads
            .borrow()
            .iter()
            .map(|(_, n)| *n as u64)
            .sum::<u64>(),
        reader.blocks[&id].payloads[2].stored_length
    );
    assert_eq!(
        reader.read_geometry(id).unwrap().points()[1].color,
        ColorRgb::new(40, 50, 60)
    );
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([100.0, 200.0]));
    assert!(reader.attribute_range(0, 1).is_err());
}

#[test]
fn split_rejects_wrong_truncated_or_corrupt_companion_without_losing_geometry() {
    let (primary, attributes) = split_bytes(source_block());
    let block = source_block();
    let other = SourcePointBlock::new(
        block.id(),
        block.positions_local().to_vec(),
        vec![ColorRgb::new(1, 2, 3); 2],
        block.attributes().to_vec(),
    );
    let (_, wrong) = split_bytes(other);
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    assert!(reader.attach_attributes(Cursor::new(wrong)).is_err());
    assert!(
        reader
            .attach_attributes(Cursor::new(attributes[..attributes.len() - 1].to_vec()))
            .is_err()
    );
    reader.read_positions(BlockId::new(42)).unwrap();
    let mut corrupt = attributes;
    corrupt[24] ^= 1;
    reader.attach_attributes(Cursor::new(corrupt)).unwrap();
    reader.read_positions(BlockId::new(42)).unwrap();
    reader.read_attribute(BlockId::new(42), 0).unwrap();
    assert!(matches!(
        reader.read_display_colors(BlockId::new(42)),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
}

#[test]
fn split_rejects_companion_directory_gaps_before_any_payload_read() {
    let (mut primary, _) = split_bytes(source_block());
    let footer = primary.len() - 40;
    let first_attribute = footer - (93 + 3 * 29) + 93 + 29;
    primary[first_attribute..first_attribute + 8].copy_from_slice(&25_u64.to_le_bytes());
    repair_index_and_footer_checksums(&mut primary);
    assert!(FormatReader::new(Cursor::new(primary)).is_err());
}

#[test]
fn finite_ranges_are_derived_per_node_and_combined_across_lod() {
    let base = metadata();
    let bounds = base.dataset().hierarchy().stats().root_bounds;
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(0),
                None,
                0,
                bounds,
                2,
                0.25,
                vec![BlockId::new(42)],
            )
            .unwrap(),
            SpatialNode::new(
                NodeId::new(1),
                Some(NodeId::new(0)),
                1,
                bounds,
                2,
                0.1,
                vec![BlockId::new(43)],
            )
            .unwrap(),
        ],
        base.dataset().hierarchy().attributes().to_vec(),
        Some(4),
        HierarchySemantics::Additive,
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(base.dataset().coordinate_frame(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let mut writer = FormatWriter::new(
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.001, 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(source_block()).unwrap();
    let base = source_block();
    let descriptor = base.attributes()[0].descriptor().clone();
    let column = AttributeColumn::new(descriptor, AttributeValues::U16(vec![500, 600]), 2).unwrap();
    writer
        .write_block(SourcePointBlock::new(
            BlockId::new(43),
            base.positions_local().to_vec(),
            base.colors().to_vec(),
            vec![column],
        ))
        .unwrap();
    let reader = FormatReader::new(writer.finish().unwrap()).unwrap();
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([100.0, 600.0]));
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([100.0, 600.0]));
    assert_eq!(
        reader
            .format_metadata()
            .node_attribute_ranges(NodeId::new(0))[0]
            .max(),
        &AttributeValues::U16(vec![200])
    );
    assert_eq!(
        reader
            .format_metadata()
            .node_attribute_ranges(NodeId::new(1))[0]
            .min(),
        &AttributeValues::U16(vec![500])
    );
}

#[test]
fn attribute_range_cache_validates_before_lookup_and_memoizes_none() {
    let reader = FormatReader::new(Cursor::new(format_bytes())).unwrap();
    assert!(matches!(
        reader.attribute_range(1, 0),
        Err(FormatError::UnknownAttribute(1))
    ));
    assert!(matches!(
        reader.attribute_range(0, 1),
        Err(FormatError::InvalidSchema)
    ));
    assert!(reader.attribute_range_cache.lock().unwrap().is_empty());

    let empty = FormatReader::new(Cursor::new(empty_attribute_range_bytes())).unwrap();
    assert_eq!(empty.attribute_range(0, 0).unwrap(), None);
    assert_eq!(empty.attribute_range(0, 0).unwrap(), None);
    let cache = empty.attribute_range_cache.lock().unwrap();
    assert_eq!(cache.len(), 1);
    assert_eq!(cache[0].range, None);
}

#[test]
fn attribute_range_cache_preserves_multicomponent_integer_ranges_and_evicts_lru() {
    const COMPONENTS: u32 = 65;
    let reader = FormatReader::new(Cursor::new(multicomponent_range_bytes())).unwrap();
    assert_eq!(
        reader.attribute_range(0, 0).unwrap(),
        Some([(u64::MAX - 1) as f64, u64::MAX as f64])
    );
    assert_eq!(reader.attribute_range(0, 1).unwrap(), Some([7.0, 11.0]));

    for component in 0..64 {
        reader.attribute_range(0, component).unwrap();
    }
    // Refresh component zero so component one becomes the least recently used entry.
    reader.attribute_range(0, 0).unwrap();
    reader.attribute_range(0, COMPONENTS - 1).unwrap();

    let cache = reader.attribute_range_cache.lock().unwrap();
    assert_eq!(cache.len(), 64);
    assert_eq!(cache.front().unwrap().component, 2);
    assert!(!cache.iter().any(|entry| entry.component == 1));
    assert!(cache.iter().any(|entry| entry.component == 0));
    assert!(cache.iter().any(|entry| entry.component == COMPONENTS - 1));
    drop(cache);
    assert_eq!(reader.attribute_range(0, 1).unwrap(), Some([7.0, 11.0]));
    assert_eq!(reader.attribute_range_cache.lock().unwrap().len(), 64);
}
