use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
};

use las::{PointDataBuilder, Reader};
use serde::Deserialize;
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeValues, BlockId, ColorRgb, CoordinateFrame,
    DatasetMetadata, HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatWriter, FormatWriterOptions, OctreeWriter, SourcePointBlock,
};

use super::{
    ConversionProgress, ConversionReport, ConversionStage, ConvertError, ConvertOptions,
    MAX_BLOCKS, create_temporary_pair, descriptor, emit_progress, input_batch_point_limit,
    omitted_dimensions, output_attributes_path, publish_output_pair, source_schema,
};

const MAX_EPT_NODES: usize = 1_000_000;
const MAX_EPT_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_EPT_HIERARCHY_PAGE_BYTES: u64 = 64 * 1024 * 1024;
const ESTIMATED_EPT_NODE_BYTES: u64 = 768;
const ESTIMATED_EPT_BLOCK_BYTES: u64 = 512;

#[derive(Deserialize)]
struct EptManifest {
    #[serde(rename = "dataType")]
    data_type: String,
    #[serde(rename = "hierarchyType")]
    hierarchy_type: String,
    points: u64,
    bounds: [f64; 6],
    schema: serde_json::Value,
    #[serde(default)]
    srs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EptKey {
    depth: u32,
    x: u32,
    y: u32,
    z: u32,
}

impl EptKey {
    fn parse(value: &str) -> Result<Self, ConvertError> {
        let values = value
            .split('-')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ConvertError::InvalidEpt(format!("invalid hierarchy key {value:?}")))?;
        let [depth, x, y, z] = values.as_slice() else {
            return Err(ConvertError::InvalidEpt(format!(
                "hierarchy key must have four integers: {value:?}"
            )));
        };
        if *depth > 30 {
            return Err(ConvertError::InvalidEpt(format!(
                "hierarchy depth exceeds 30 in key {value:?}"
            )));
        }
        let side = 1_u32 << depth;
        if *x >= side || *y >= side || *z >= side {
            return Err(ConvertError::InvalidEpt(format!(
                "hierarchy coordinates exceed depth in key {value:?}"
            )));
        }
        Ok(Self {
            depth: *depth,
            x: *x,
            y: *y,
            z: *z,
        })
    }

    fn name(self) -> String {
        format!("{}-{}-{}-{}", self.depth, self.x, self.y, self.z)
    }

    fn parent(self) -> Option<Self> {
        (self.depth > 0).then(|| Self {
            depth: self.depth - 1,
            x: self.x / 2,
            y: self.y / 2,
            z: self.z / 2,
        })
    }
}

struct EptNode {
    key: EptKey,
    id: NodeId,
    parent: Option<NodeId>,
    point_count: u64,
    bounds: SpatialBounds,
    path: Option<PathBuf>,
    block_ids: Vec<BlockId>,
}

pub(super) fn is_ept_input(path: &Path) -> bool {
    path.is_dir() || path.file_name().is_some_and(|name| name == "ept.json")
}

pub(super) fn convert(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
) -> Result<ConversionReport, ConvertError> {
    if output_path.exists() {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }
    let attributes_output_path = output_attributes_path(output_path);
    if attributes_output_path.exists() {
        return Err(ConvertError::OutputExists(attributes_output_path));
    }
    if options.spatial_divisions() != 2
        || (options.spatial_workers_explicit() && options.spatial_workers() != 1)
    {
        return Err(ConvertError::UnsupportedEpt(
            "spatial partition options apply to LAS/LAZ input; EPT topology is preserved"
                .to_owned(),
        ));
    }

    let manifest_path = if input_path.is_dir() {
        input_path.join("ept.json")
    } else {
        input_path.to_owned()
    };
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    ensure_file_size(&manifest_path, MAX_EPT_MANIFEST_BYTES, "ept.json")?;
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::InspectingInput,
            points_processed: 0,
            total_points: 0,
            bytes_written: 0,
        },
        0,
    )?;
    let manifest: EptManifest =
        serde_json::from_reader(BufReader::new(File::open(&manifest_path)?))
            .map_err(|error| ConvertError::InvalidEpt(format!("cannot parse ept.json: {error}")))?;
    if manifest.data_type != "laszip" {
        return Err(ConvertError::UnsupportedEpt(format!(
            "dataType {:?}; this version supports only laszip",
            manifest.data_type
        )));
    }
    if manifest.hierarchy_type != "json" {
        return Err(ConvertError::UnsupportedEpt(format!(
            "hierarchyType {:?}; this version supports only local JSON hierarchy pages",
            manifest.hierarchy_type
        )));
    }
    validate_schema(&manifest.schema)?;
    let bounds = SpatialBounds::new(
        [manifest.bounds[0], manifest.bounds[1], manifest.bounds[2]],
        [manifest.bounds[3], manifest.bounds[4], manifest.bounds[5]],
    )
    .map_err(|_| ConvertError::InvalidEpt("bounds are invalid".to_owned()))?;
    let frame = CoordinateFrame::new(bounds.min())?;
    // Even an empty EPT incurs at least one point of staging capacity and fixed overhead
    // in the existing estimate. Reject impossible budgets before parsing hierarchy pages.
    check_ept_memory_budget(1, 0, 0, options.max_memory_budget_bytes())?;
    let counts = read_hierarchy(root, options.max_memory_budget_bytes())?;
    let root_key = EptKey {
        depth: 0,
        x: 0,
        y: 0,
        z: 0,
    };
    if !counts.contains_key(&root_key) {
        return Err(ConvertError::InvalidEpt(
            "hierarchy has no root node 0-0-0-0".to_owned(),
        ));
    }

    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::PlanningMetadata,
            points_processed: 0,
            total_points: manifest.points,
            bytes_written: 0,
        },
        0,
    )?;
    let total = counts.values().try_fold(0_u64, |sum, count| {
        sum.checked_add(*count).ok_or(ConvertError::SizeOverflow)
    })?;
    if total != manifest.points {
        return Err(ConvertError::InvalidEpt(format!(
            "hierarchy sums to {total} points but ept.json declares {}",
            manifest.points
        )));
    }
    // Grow these planning structures only as each node passes the memory preflight below.
    let mut nodes = Vec::new();
    let mut expected_format: Option<las::point::Format> = None;
    let mut schema = None;
    let mut omitted = Vec::new();
    let mut next_block = 0_u64;
    let mut all_blocks = Vec::new();
    let mut memory_estimate = 0_u64;
    for (index, (&key, &count)) in counts.iter().enumerate() {
        let node_bounds = cell_bounds(bounds, key)?;
        let file_path = root.join("ept-data").join(format!("{}.laz", key.name()));
        let mut point_format = None;
        if count > 0 {
            if !file_path.is_file() {
                return Err(ConvertError::InvalidEpt(format!(
                    "node {} declares {count} points but {} is missing",
                    key.name(),
                    file_path.display()
                )));
            }
            let reader = Reader::from_path(&file_path)?;
            let header = reader.header();
            if header.number_of_points() != count {
                return Err(ConvertError::InvalidEpt(format!(
                    "node {} declares {count} points but its LAZ header declares {}",
                    key.name(),
                    header.number_of_points()
                )));
            }
            let format = *header.point_format();
            if expected_format.is_some_and(|expected| {
                expected.has_gps_time != format.has_gps_time
                    || expected.has_color != format.has_color
                    || expected.has_nir != format.has_nir
                    || expected.is_extended != format.is_extended
                    || expected.has_waveform != format.has_waveform
                    || expected.extra_bytes != format.extra_bytes
            }) {
                return Err(ConvertError::InvalidEpt(
                    "node LAZ files use inconsistent point formats".to_owned(),
                ));
            }
            if expected_format.is_none() {
                omitted = omitted_dimensions(&format, header.has_crs_vlrs());
                schema = Some(source_schema(&format));
                expected_format = Some(format);
            }
            point_format = Some(format);
        }
        let record_length = point_format.map_or(34, |format| usize::from(format.len()).max(1));
        let point_limit = input_batch_point_limit(options.points_per_block(), record_length);
        let block_count = count.div_ceil(point_limit);
        let planned_block_count = next_block
            .checked_add(block_count)
            .ok_or(ConvertError::SizeOverflow)?;
        if planned_block_count > MAX_BLOCKS {
            return Err(ConvertError::TooManyBlocks(planned_block_count));
        }
        memory_estimate = check_ept_memory_budget(
            input_batch_point_limit(
                options.points_per_block(),
                expected_format.map_or(record_length, |format| usize::from(format.len())),
            ),
            u64::try_from(nodes.len())
                .map_err(|_| ConvertError::SizeOverflow)?
                .checked_add(1)
                .ok_or(ConvertError::SizeOverflow)?,
            planned_block_count,
            options.max_memory_budget_bytes(),
        )?;
        let block_ids = (0..block_count)
            .map(|_| {
                let id = BlockId::new(next_block);
                next_block += 1;
                id
            })
            .collect::<Vec<_>>();
        all_blocks.extend(block_ids.iter().copied());
        nodes.push(EptNode {
            key,
            id: NodeId::new(u64::try_from(index).map_err(|_| ConvertError::SizeOverflow)?),
            parent: None,
            point_count: count,
            bounds: node_bounds,
            path: (count > 0).then_some(file_path),
            block_ids,
        });
    }
    let schema = schema.unwrap_or_default();
    let node_ids = nodes
        .iter()
        .map(|node| (node.key, node.id))
        .collect::<BTreeMap<_, _>>();
    for node in &mut nodes {
        node.parent = node
            .key
            .parent()
            .map(|parent| {
                node_ids.get(&parent).copied().ok_or_else(|| {
                    ConvertError::InvalidEpt(format!(
                        "node {} has no parent {}",
                        node.key.name(),
                        parent.name()
                    ))
                })
            })
            .transpose()?;
    }
    let spatial_nodes = nodes
        .iter()
        .map(|node| {
            SpatialNode::new(
                node.id,
                node.parent,
                node.key.depth,
                node.bounds,
                node.point_count,
                node_diagonal(node.bounds),
                node.block_ids.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let hierarchy = SpatialHierarchy::new_with_semantics(
        spatial_nodes,
        schema.clone(),
        Some(manifest.points),
        HierarchySemantics::Additive,
    )?;
    let crs = manifest
        .srs
        .and_then(|srs| serde_json::to_string(&srs).ok());
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(frame, hierarchy),
        crs,
        Some("EPT additive hierarchy; node payloads preserve source points; geometric error is the node-cell diagonal".to_owned()),
        Vec::new(),
    )?
    .with_visual_scale(super::visual_scale_for_bounds(bounds)?)?;
    let mut temporary = create_temporary_pair(output_path)?;
    let file = temporary
        .geometry
        .file
        .take()
        .expect("temporary file is present");
    let attributes_file = temporary
        .attributes
        .file
        .take()
        .expect("temporary companion file exists");
    let mut writer = FormatWriter::new_split_for_version(
        options.format_version(),
        file,
        attributes_file,
        metadata,
        FormatWriterOptions::new(options.max_quantization_error(), 64 * 1024 * 1024)
            .map_err(ConvertError::Format)?,
    )?;
    let mut processed = 0_u64;
    for node in &nodes {
        let Some(path) = &node.path else {
            continue;
        };
        let mut reader = Reader::from_path(path)?;
        let point_format = *reader.header().point_format();
        let point_limit =
            input_batch_point_limit(options.points_per_block(), usize::from(point_format.len()));
        let mut point_data = PointDataBuilder::new().for_header(reader.header()).build();
        let mut node_processed = 0_u64;
        for block_id in &node.block_ids {
            let wanted = (node.point_count - node_processed).min(point_limit);
            let read = reader.fill_points(wanted, &mut point_data)?;
            if read != wanted {
                return Err(ConvertError::PointCountMismatch {
                    expected: node.point_count,
                    actual: node_processed + read,
                });
            }
            let block = point_data_to_block(
                *block_id,
                &point_data,
                frame,
                point_format,
                node.bounds,
                node.key,
            )?;
            OctreeWriter::write_block(&mut writer, block)?;
            node_processed += read;
            processed += read;
            emit_progress(
                on_progress,
                ConversionProgress {
                    stage: ConversionStage::ReadingAndWritingBlocks,
                    points_processed: processed,
                    total_points: manifest.points,
                    bytes_written: fs::metadata(&temporary.geometry.path)?.len(),
                },
                processed,
            )?;
        }
        if node_processed != node.point_count {
            return Err(ConvertError::PointCountMismatch {
                expected: node.point_count,
                actual: node_processed,
            });
        }
    }
    if processed != manifest.points {
        return Err(ConvertError::PointCountMismatch {
            expected: manifest.points,
            actual: processed,
        });
    }
    let (file, attributes) = OctreeWriter::finish_pair(writer)?;
    file.sync_all()?;
    let attributes = attributes.expect("split writer returns companion");
    attributes.sync_all()?;
    drop(file);
    drop(attributes);
    let file_length = fs::metadata(&temporary.geometry.path)?.len();
    let attributes_temporary_path = temporary.attributes.path.clone();
    let attributes_length = fs::metadata(&attributes_temporary_path)?.len();
    let validation = super::OutputValidation {
        total_points: manifest.points,
        block_ids: &all_blocks,
        source_block_ids: &all_blocks,
        file_length,
        attributes_length,
        requested_workers: options.spatial_workers(),
        memory_budget_bytes: options.max_memory_budget_bytes(),
        reserved_memory_bytes: 0,
    };
    let (output_bytes, output_validation) = publish_output_pair(
        &temporary.geometry.path,
        &attributes_temporary_path,
        output_path,
        validation,
        on_progress,
        None,
    )?;
    let _ = on_progress(ConversionProgress {
        stage: ConversionStage::Complete,
        points_processed: manifest.points,
        total_points: manifest.points,
        bytes_written: output_bytes,
    });
    Ok(ConversionReport {
        point_count: manifest.points,
        block_count: all_blocks.len() as u64,
        output_bytes,
        estimated_peak_memory_bytes: memory_estimate,
        omitted_dimensions: omitted,
        block_preparation: None,
        output_validation,
    })
}

fn point_data_to_block(
    id: BlockId,
    data: &las::PointData,
    frame: CoordinateFrame,
    format: las::point::Format,
    node_bounds: SpatialBounds,
    key: EptKey,
) -> Result<SourcePointBlock, ConvertError> {
    let positions: Vec<[f64; 3]> = data
        .x()
        .zip(data.y())
        .zip(data.z())
        .map(|((x, y), z)| {
            let position = [x, y, z];
            if position.iter().any(|coordinate| !coordinate.is_finite()) {
                return Err(ConvertError::InvalidEpt(
                    "node contains non-finite coordinates".to_owned(),
                ));
            }
            if (0..3).any(|axis| {
                position[axis] < node_bounds.min()[axis] || position[axis] > node_bounds.max()[axis]
            }) {
                return Err(ConvertError::InvalidEpt(format!(
                    "point in node {} lies outside its spatial cell",
                    key.name()
                )));
            }
            Ok(std::array::from_fn(|axis| {
                position[axis] - frame.origin()[axis]
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let positions = positions.into_iter().collect::<Vec<_>>();
    let count = positions.len();
    let intensities = data.intensity().collect::<Vec<_>>();
    let rgb = data
        .rgb()
        .map(|values| {
            values
                .map(|(red, green, blue)| [red, green, blue])
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let colors16 = format.has_color.then_some(rgb);
    let colors = if let Some(rgb) = &colors16 {
        rgb.iter()
            .map(|value| {
                ColorRgb::new(
                    (value[0] >> 8) as u8,
                    (value[1] >> 8) as u8,
                    (value[2] >> 8) as u8,
                )
            })
            .collect()
    } else {
        intensities
            .iter()
            .map(|intensity| {
                let gray = (intensity >> 8) as u8;
                ColorRgb::new(gray, gray, gray)
            })
            .collect()
    };
    let mut attributes = vec![
        AttributeColumn::new(
            descriptor("intensity", AttributeDataType::U16, 1),
            AttributeValues::U16(intensities),
            count,
        )?,
        AttributeColumn::new(
            descriptor("classification", AttributeDataType::U8, 1),
            AttributeValues::U8(data.classification().collect()),
            count,
        )?,
        AttributeColumn::new(
            descriptor("return_number", AttributeDataType::U8, 1),
            AttributeValues::U8(data.return_number().collect()),
            count,
        )?,
        AttributeColumn::new(
            descriptor("number_of_returns", AttributeDataType::U8, 1),
            AttributeValues::U8(data.number_of_returns().collect()),
            count,
        )?,
        AttributeColumn::new(
            descriptor("scan_angle_degrees", AttributeDataType::F32, 1),
            AttributeValues::F32(data.scan_angle_degrees().collect()),
            count,
        )?,
        AttributeColumn::new(
            descriptor("user_data", AttributeDataType::U8, 1),
            AttributeValues::U8(data.user_data().collect()),
            count,
        )?,
        AttributeColumn::new(
            descriptor("point_source_id", AttributeDataType::U16, 1),
            AttributeValues::U16(data.point_source_id().collect()),
            count,
        )?,
    ];
    if format.has_gps_time {
        attributes.push(AttributeColumn::new(
            descriptor("gps_time", AttributeDataType::F64, 1),
            AttributeValues::F64(
                data.gps_time()
                    .ok_or(ConvertError::InvalidAttributeSchema)?
                    .collect(),
            ),
            count,
        )?);
    }
    if format.has_color {
        let values = colors16
            .ok_or(ConvertError::InvalidAttributeSchema)?
            .into_iter()
            .flatten()
            .collect();
        attributes.push(AttributeColumn::new(
            descriptor("rgb", AttributeDataType::U16, 3),
            AttributeValues::U16(values),
            count,
        )?);
    }
    if format.has_nir {
        attributes.push(AttributeColumn::new(
            descriptor("nir", AttributeDataType::U16, 1),
            AttributeValues::U16(
                data.nir()
                    .ok_or(ConvertError::InvalidAttributeSchema)?
                    .collect(),
            ),
            count,
        )?);
    }
    Ok(SourcePointBlock::new(id, positions, colors, attributes))
}

fn read_hierarchy(
    root: &Path,
    memory_budget_bytes: u64,
) -> Result<BTreeMap<EptKey, u64>, ConvertError> {
    let hierarchy_dir = root.join("ept-hierarchy");
    let mut counts = BTreeMap::new();
    let mut pages = BTreeSet::new();
    let mut loaded_pages = BTreeSet::new();
    let mut positive_node_count = 0_u64;
    pages.insert(EptKey {
        depth: 0,
        x: 0,
        y: 0,
        z: 0,
    });
    while let Some(page_key) = pages.pop_first() {
        if !loaded_pages.insert(page_key) {
            continue;
        }
        let page_path = hierarchy_dir.join(format!("{}.json", page_key.name()));
        ensure_file_size(
            &page_path,
            MAX_EPT_HIERARCHY_PAGE_BYTES,
            "EPT hierarchy page",
        )?;
        let page: BTreeMap<String, i64> =
            serde_json::from_reader(BufReader::new(File::open(&page_path)?)).map_err(|error| {
                ConvertError::InvalidEpt(format!(
                    "cannot parse hierarchy page {}: {error}",
                    page_path.display()
                ))
            })?;
        match page.get(&page_key.name()) {
            Some(count) if *count >= 0 => {}
            Some(_) => {
                return Err(ConvertError::InvalidEpt(format!(
                    "hierarchy page {} marks its own root as a page reference",
                    page_path.display()
                )));
            }
            None => {
                return Err(ConvertError::InvalidEpt(format!(
                    "hierarchy page {} does not contain its root node",
                    page_path.display()
                )));
            }
        }
        for (key_text, signed_count) in page {
            let key = EptKey::parse(&key_text)?;
            if signed_count < -1 {
                return Err(ConvertError::InvalidEpt(format!(
                    "invalid count {signed_count} for node {key_text}"
                )));
            }
            if signed_count == -1 {
                if loaded_pages.contains(&key) || !pages.insert(key) {
                    continue;
                }
                if pages.len() + loaded_pages.len() + counts.len() > MAX_EPT_NODES {
                    return Err(ConvertError::InvalidEpt(
                        "hierarchy exceeds the node limit".to_owned(),
                    ));
                }
                check_ept_hierarchy_memory_budget(
                    counts.len(),
                    pages.len(),
                    loaded_pages.len(),
                    positive_node_count,
                    memory_budget_bytes,
                )?;
                continue;
            }
            let count = signed_count as u64;
            match counts.insert(key, count) {
                Some(previous) if previous != count => {
                    return Err(ConvertError::InvalidEpt(format!(
                        "conflicting counts for node {key_text}"
                    )));
                }
                Some(_) => {}
                None if count > 0 => {
                    positive_node_count = positive_node_count
                        .checked_add(1)
                        .ok_or(ConvertError::SizeOverflow)?;
                }
                None => {}
            }
            if counts.len() > MAX_EPT_NODES {
                return Err(ConvertError::InvalidEpt(
                    "hierarchy exceeds the node limit".to_owned(),
                ));
            }
            check_ept_hierarchy_memory_budget(
                counts.len(),
                pages.len(),
                loaded_pages.len(),
                positive_node_count,
                memory_budget_bytes,
            )?;
        }
    }
    Ok(counts)
}

fn check_ept_memory_budget(
    point_batch_limit: u64,
    node_count: u64,
    block_count: u64,
    limit: u64,
) -> Result<u64, ConvertError> {
    let requested = point_batch_limit
        .saturating_mul(192)
        .saturating_add(node_count.saturating_mul(ESTIMATED_EPT_NODE_BYTES))
        .saturating_add(block_count.saturating_mul(ESTIMATED_EPT_BLOCK_BYTES))
        .saturating_add(16 * 1024 * 1024);
    if requested > limit {
        return Err(ConvertError::MemoryBudgetExceeded { requested, limit });
    }
    Ok(requested)
}

fn check_ept_hierarchy_memory_budget(
    known_count_nodes: usize,
    pending_page_nodes: usize,
    loaded_page_nodes: usize,
    positive_count_nodes: u64,
    limit: u64,
) -> Result<(), ConvertError> {
    let page_node_count = u64::try_from(pending_page_nodes)
        .map_err(|_| ConvertError::SizeOverflow)?
        .checked_add(u64::try_from(loaded_page_nodes).map_err(|_| ConvertError::SizeOverflow)?)
        .ok_or(ConvertError::SizeOverflow)?;
    let known_node_count = u64::try_from(known_count_nodes)
        .map_err(|_| ConvertError::SizeOverflow)?
        .max(page_node_count);
    check_ept_memory_budget(1, known_node_count, positive_count_nodes, limit)?;
    Ok(())
}

fn validate_schema(schema: &serde_json::Value) -> Result<(), ConvertError> {
    let fields = schema
        .as_array()
        .ok_or_else(|| ConvertError::InvalidEpt("schema must be a JSON array".to_owned()))?;
    let mut names = BTreeSet::new();
    for field in fields {
        let object = field.as_object().ok_or_else(|| {
            ConvertError::InvalidEpt("schema entries must be JSON objects".to_owned())
        })?;
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ConvertError::InvalidEpt("schema field has no string name".to_owned())
            })?;
        let data_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ConvertError::InvalidEpt(format!("schema field {name:?} has no string type"))
            })?;
        let size = object
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .filter(|size| *size > 0)
            .ok_or_else(|| {
                ConvertError::InvalidEpt(format!("schema field {name:?} has no positive size"))
            })?;
        if name.is_empty()
            || !matches!(data_type, "signed" | "unsigned" | "float" | "floating")
            || size > 8
            || !names.insert(name)
        {
            return Err(ConvertError::InvalidEpt(format!(
                "invalid or duplicate EPT schema field {name:?}"
            )));
        }
    }
    if !["X", "Y", "Z"].into_iter().all(|axis| names.contains(axis)) {
        return Err(ConvertError::InvalidEpt(
            "schema must describe X, Y, and Z coordinates".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_file_size(path: &Path, limit: u64, description: &str) -> Result<(), ConvertError> {
    let size = fs::metadata(path)?.len();
    if size > limit {
        return Err(ConvertError::InvalidEpt(format!(
            "{description} is {size} bytes, exceeding the {limit}-byte limit"
        )));
    }
    Ok(())
}

fn cell_bounds(root: SpatialBounds, key: EptKey) -> Result<SpatialBounds, ConvertError> {
    let divisor = f64::from(1_u32 << key.depth);
    let min = std::array::from_fn(|axis| {
        root.min()[axis]
            + (root.max()[axis] - root.min()[axis]) * f64::from([key.x, key.y, key.z][axis])
                / divisor
    });
    let max = std::array::from_fn(|axis| {
        root.min()[axis]
            + (root.max()[axis] - root.min()[axis]) * f64::from([key.x, key.y, key.z][axis] + 1)
                / divisor
    });
    SpatialBounds::new(min, max)
        .map_err(|_| ConvertError::InvalidEpt("node bounds are invalid".to_owned()))
}

fn node_diagonal(bounds: SpatialBounds) -> f64 {
    let size: [f64; 3] = std::array::from_fn(|axis| bounds.max()[axis] - bounds.min()[axis]);
    size[0].hypot(size[1]).hypot(size[2])
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use las::{Builder, Point, Writer, point::Format};
    use viewer_core::{DatasetSource, HierarchySemantics, NodeId};
    use viewer_format::{FormatReader, OctreeReader};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "viewer-ept-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_laz(path: &Path, position: [f64; 3]) {
        let mut builder = Builder::from((1, 2));
        builder.point_format = Format::new(0).unwrap();
        builder.point_format.is_compressed = true;
        let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
        writer
            .write_point(Point {
                x: position[0],
                y: position[1],
                z: position[2],
                intensity: 0xAB00,
                classification: las::point::Classification::Ground,
                ..Point::default()
            })
            .unwrap();
    }

    fn write_manifest(root: &Path, data_type: &str, points: u64) {
        fs::write(
            root.join("ept.json"),
            format!(
                r#"{{"dataType":"{data_type}","hierarchyType":"json","points":{points},"bounds":[0,0,0,8,8,8],"schema":[{{"name":"X","type":"signed","size":4}},{{"name":"Y","type":"signed","size":4}},{{"name":"Z","type":"signed","size":4}},{{"name":"ScanAngleRank","type":"float","size":4}},{{"name":"GpsTime","type":"float","size":8}}]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn schema_keeps_legacy_floating_alias_and_rejects_unknown_types() {
        for data_type in ["float", "floating", "unknown"] {
            let schema = serde_json::json!([
                { "name": "X", "type": "signed", "size": 4 },
                { "name": "Y", "type": "signed", "size": 4 },
                { "name": "Z", "type": "signed", "size": 4 },
                { "name": "GpsTime", "type": data_type, "size": 8 }
            ]);
            assert_eq!(validate_schema(&schema).is_ok(), data_type != "unknown");
        }
    }

    #[test]
    fn converts_local_laszip_ept_pages_as_additive_hierarchy() {
        for version in [
            viewer_format::FormatVersion::V6_PRE1,
            viewer_format::FormatVersion::V6_PRE2,
            viewer_format::FormatVersion::V6_PRE3,
            viewer_format::FormatVersion::V6_PRE4,
        ] {
            let root = dir();
            fs::create_dir_all(root.join("ept-data")).unwrap();
            fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
            write_manifest(&root, "laszip", 3);
            fs::write(
                root.join("ept-hierarchy/0-0-0-0.json"),
                r#"{"0-0-0-0":1,"1-0-0-0":-1}"#,
            )
            .unwrap();
            fs::write(
                root.join("ept-hierarchy/1-0-0-0.json"),
                r#"{"1-0-0-0":1,"2-0-0-0":1}"#,
            )
            .unwrap();
            write_laz(&root.join("ept-data/0-0-0-0.laz"), [7.0, 7.0, 7.0]);
            write_laz(&root.join("ept-data/1-0-0-0.laz"), [3.0, 3.0, 3.0]);
            write_laz(&root.join("ept-data/2-0-0-0.laz"), [1.0, 1.0, 1.0]);

            let output = root.join("result.oct");
            let mut progress = Vec::new();
            let options = ConvertOptions::default()
                .with_format_version(version)
                .unwrap();
            let report = crate::convert_file_with_progress(&root, &output, options, |event| {
                progress.push(event);
                true
            })
            .unwrap();
            assert_eq!(report.point_count, 3);
            assert_eq!(report.block_preparation, None);
            assert!(report.output_validation.worker_limit > 0);
            assert_eq!(report.block_count, 3);
            assert_eq!(
                progress.first().unwrap().stage,
                ConversionStage::InspectingInput
            );
            assert_eq!(progress.last().unwrap().stage, ConversionStage::Complete);

            let mut reader = FormatReader::new(File::open(&output).unwrap()).unwrap();
            assert_eq!(reader.format_version(), version);
            reader
                .attach_attributes(File::open(output_attributes_path(&output)).unwrap())
                .unwrap();
            let hierarchy = reader.metadata().hierarchy();
            assert_eq!(reader.format_metadata().visual_scale(), 12.5);
            assert_eq!(hierarchy.semantics(), HierarchySemantics::Additive);
            assert_eq!(hierarchy.stats().node_count, 3);
            assert_eq!(hierarchy.stats().source_point_count, Some(3));
            assert_eq!(hierarchy.node(NodeId::new(0)).unwrap().point_count(), 1);
            assert_eq!(hierarchy.node(NodeId::new(1)).unwrap().point_count(), 1);
            assert_eq!(hierarchy.node(NodeId::new(2)).unwrap().point_count(), 1);
            let block = reader.read_block(BlockId::new(2)).unwrap();
            assert_eq!(block.points().len(), 1);
            let viewer_core::AttributeValues::U16(intensity) = block.attributes()[0].values()
            else {
                panic!("EPT intensity must remain u16");
            };
            assert_eq!(intensity, &[0xAB00]);
            let viewer_core::AttributeValues::U8(classification) = block.attributes()[1].values()
            else {
                panic!("EPT classification must remain u8");
            };
            assert_eq!(classification, &[2]);
        }
    }

    #[test]
    fn rejects_unsupported_ept_codec_before_reading_hierarchy() {
        let root = dir();
        write_manifest(&root, "zstandard", 0);
        assert!(matches!(
            convert(&root, &root.join("result.oct"), ConvertOptions::default(), &mut |_| true),
            Err(ConvertError::UnsupportedEpt(message)) if message.contains("laszip")
        ));
    }

    #[test]
    fn rejects_checkpoint_and_repartition_options_for_ept() {
        let root = dir();
        write_manifest(&root, "laszip", 0);
        let output = root.join("result.oct");
        assert!(matches!(
            crate::convert_file_with_checkpoint(
                &root,
                &output,
                ConvertOptions::default(),
                crate::CheckpointMode::Create,
                |_| true,
            ),
            Err(ConvertError::UnsupportedEpt(message)) if message.contains("checkpoint")
        ));
        let repartitioned = ConvertOptions::default().with_spatial_divisions(4).unwrap();
        assert!(matches!(
            convert(&root, &output, repartitioned, &mut |_| true),
            Err(ConvertError::UnsupportedEpt(message)) if message.contains("topology")
        ));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_ept_count_mismatch_and_missing_node_files() {
        let root = dir();
        fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
        write_manifest(&root, "laszip", 2);
        fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":1}"#).unwrap();
        assert!(matches!(
            convert(&root, &root.join("result.oct"), ConvertOptions::default(), &mut |_| true),
            Err(ConvertError::InvalidEpt(message)) if message.contains("sums to 1")
        ));

        write_manifest(&root, "laszip", 1);
        assert!(matches!(
            convert(&root, &root.join("result.oct"), ConvertOptions::default(), &mut |_| true),
            Err(ConvertError::InvalidEpt(message)) if message.contains("is missing")
        ));
    }

    #[test]
    fn rejects_ept_memory_budget_before_planning_all_nodes_or_creating_output() {
        let root = dir();
        fs::create_dir_all(root.join("ept-data")).unwrap();
        fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
        write_manifest(&root, "laszip", 2);
        fs::write(
            root.join("ept-hierarchy/0-0-0-0.json"),
            r#"{"0-0-0-0":1,"1-0-0-0":1}"#,
        )
        .unwrap();
        write_laz(&root.join("ept-data/0-0-0-0.laz"), [7.0, 7.0, 7.0]);
        let output = root.join("result.oct");
        let options = ConvertOptions::with_memory_budget(1, 0.01, 1).unwrap();

        assert!(matches!(
            convert(&root, &output, options, &mut |_| true),
            Err(ConvertError::MemoryBudgetExceeded { limit: 1, .. })
        ));
        assert!(!output.exists());
        assert!(!output_attributes_path(&output).exists());
    }

    #[test]
    fn rejects_ept_memory_budget_while_reading_large_hierarchy_page() {
        let root = dir();
        fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
        write_manifest(&root, "laszip", 0);
        let mut page = serde_json::Map::new();
        page.insert("0-0-0-0".to_owned(), serde_json::json!(0));
        for index in 0..1_500_u32 {
            let x = index % 32;
            let y = (index / 32) % 32;
            let z = index / 1_024;
            page.insert(format!("5-{x}-{y}-{z}"), serde_json::json!(-1));
        }
        fs::write(
            root.join("ept-hierarchy/0-0-0-0.json"),
            serde_json::to_vec(&serde_json::Value::Object(page)).unwrap(),
        )
        .unwrap();
        let output = root.join("result.oct");
        let options = ConvertOptions::with_memory_budget(1, 0.01, 17 * 1024 * 1024).unwrap();

        assert!(matches!(
            convert(&root, &output, options, &mut |_| true),
            Err(ConvertError::MemoryBudgetExceeded {
                limit: 17_825_792,
                ..
            })
        ));
        assert!(!output.exists());
        assert!(!output_attributes_path(&output).exists());
    }
}
