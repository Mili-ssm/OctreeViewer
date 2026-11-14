use viewer_core::{BlockId, PointBlock};
use viewer_format::OctreeReader;
use viewer_render_wgpu::ColorMode;

/// Loads only the payloads required by the current presentation mode.
pub(crate) fn load_render_block<E: std::fmt::Display>(
    reader: &mut dyn OctreeReader<Error = E>,
    id: BlockId,
    mode: Option<ColorMode>,
    selected_attribute: Option<(u32, u32)>,
    selected_range: Option<[f64; 2]>,
) -> Result<PointBlock, String> {
    let mut geometry = if selected_attribute.is_none() && mode == Some(ColorMode::Rgb) {
        reader.read_geometry(id)
    } else {
        reader.read_positions(id)
    }
    .map_err(|error| error.to_string())?;
    if let Some((index, component)) = selected_attribute {
        let column = reader
            .read_attribute(id, index)
            .map_err(|e| e.to_string())?;
        geometry =
            viewer_render_wgpu::colorize_attribute(geometry, &column, component, selected_range)
                .map_err(str::to_owned)?;
    }
    let requested_attribute = match mode {
        Some(ColorMode::Intensity) => Some("intensity"),
        Some(ColorMode::Classification) => Some("classification"),
        _ => None,
    };
    let mut attributes = Vec::new();
    if reader.has_attributes()
        && let Some(name) = requested_attribute
        && let Some(index) = reader
            .metadata()
            .hierarchy()
            .attributes()
            .iter()
            .position(|descriptor| descriptor.name() == name)
    {
        let index = u32::try_from(index).map_err(|_| "attribute schema is too large".to_owned())?;
        attributes.push(
            reader
                .read_attribute(id, index)
                .map_err(|e| e.to_string())?,
        );
    }
    Ok(PointBlock::new(id, geometry, attributes))
}

/// Loads a block for the cells path: positions and RGB plus the intensity and classification
/// columns in every colour mode (R3-Q32), so the cells carry them and a mode change never
/// rebuilds them. The CPU path keeps loading only what its mode needs ([`load_render_block`]).
pub(crate) fn load_cell_block<E: std::fmt::Display>(
    reader: &mut dyn OctreeReader<Error = E>,
    id: BlockId,
    paint: Option<crate::cells_glue::CellPaint>,
) -> Result<PointBlock, String> {
    let mut geometry = reader
        .read_geometry(id)
        .map_err(|error| error.to_string())?;
    // An attribute colour mode paints the points before the cells are built from them (R3-Q33).
    if let Some(((index, component), range)) = paint {
        let column = reader
            .read_attribute(id, index)
            .map_err(|e| e.to_string())?;
        geometry = viewer_render_wgpu::colorize_attribute(geometry, &column, component, range)
            .map_err(str::to_owned)?;
    }
    let mut attributes = Vec::new();
    if reader.has_attributes() {
        for name in ["intensity", "classification"] {
            let Some(index) = reader
                .metadata()
                .hierarchy()
                .attributes()
                .iter()
                .position(|descriptor| descriptor.name() == name)
            else {
                continue;
            };
            let index =
                u32::try_from(index).map_err(|_| "attribute schema is too large".to_owned())?;
            attributes.push(
                reader
                    .read_attribute(id, index)
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    Ok(PointBlock::new(id, geometry, attributes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, ColorRgb,
        CoordinateFrame, DatasetMetadata, DatasetSource, NodeId, Point, PointBatch, SpatialBounds,
        SpatialHierarchy, SpatialNode,
    };

    struct MemoryReader {
        metadata: DatasetMetadata,
        calls: Vec<&'static str>,
        attributes_available: bool,
        fail: Option<&'static str>,
    }

    impl MemoryReader {
        fn new() -> Self {
            let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
            let root = SpatialNode::new(
                NodeId::new(0),
                None,
                0,
                bounds,
                2,
                0.0,
                vec![BlockId::new(7)],
            )
            .unwrap();
            let schema = [
                ("intensity", AttributeDataType::U16, 1),
                ("classification", AttributeDataType::U8, 1),
                ("vector", AttributeDataType::F32, 2),
            ]
            .into_iter()
            .map(|(name, kind, components)| {
                AttributeDescriptor::new(name, kind, components).unwrap()
            })
            .collect();
            Self {
                metadata: DatasetMetadata::new(
                    CoordinateFrame::new([0.0; 3]).unwrap(),
                    SpatialHierarchy::new(vec![root], schema, Some(2)).unwrap(),
                ),
                calls: vec![],
                attributes_available: true,
                fail: None,
            }
        }
    }

    impl DatasetSource for MemoryReader {
        type Error = &'static str;
        fn metadata(&self) -> &DatasetMetadata {
            &self.metadata
        }
        fn read_block(&mut self, _: BlockId) -> Result<PointBlock, Self::Error> {
            panic!("render loading must not request complete blocks")
        }
    }

    impl OctreeReader for MemoryReader {
        fn format_version(&self) -> viewer_format::FormatVersion {
            viewer_format::FormatVersion::V5
        }

        fn has_attributes(&self) -> bool {
            self.attributes_available
        }
        fn read_positions(&mut self, _: BlockId) -> Result<PointBatch, Self::Error> {
            self.calls.push("xyz");
            if self.fail == Some("xyz") {
                return Err("positions failed");
            }
            Ok(PointBatch::new(vec![
                Point::new(
                    [0.0; 3],
                    ColorRgb::new(160, 160, 160)
                );
                2
            ]))
        }
        fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, Self::Error> {
            let points = self.read_positions(id)?;
            if !self.has_attributes() {
                return Ok(points);
            }
            let colors = self.read_display_colors(id)?;
            let mut points = points.into_points();
            for (point, color) in points.iter_mut().zip(colors) {
                point.color = color;
            }
            Ok(PointBatch::new(points))
        }
        fn read_display_colors(&mut self, _: BlockId) -> Result<Vec<ColorRgb>, Self::Error> {
            self.calls.push("rgb");
            Ok(vec![ColorRgb::new(10, 20, 30); 2])
        }
        fn read_attribute(
            &mut self,
            _: BlockId,
            index: u32,
        ) -> Result<AttributeColumn, Self::Error> {
            if self.fail == Some("column") {
                return Err("column failed");
            }
            let (name, values) = match index {
                0 => ("intensity", AttributeValues::U16(vec![100, 200])),
                1 => ("classification", AttributeValues::U8(vec![2, 6])),
                2 => (
                    "vector",
                    AttributeValues::F32(vec![999.0, -1.0, 999.0, 1.0]),
                ),
                _ => return Err("unknown attribute"),
            };
            self.calls.push(name);
            Ok(AttributeColumn::new(
                self.metadata.hierarchy().attributes()[index as usize].clone(),
                values,
                2,
            )
            .unwrap())
        }
        fn read_attribute_row(
            &mut self,
            id: BlockId,
            row: u64,
            index: u32,
        ) -> Result<AttributeValues, Self::Error> {
            let column = self.read_attribute(id, index)?;
            column
                .values()
                .select_rows(
                    &[row as usize],
                    column.descriptor().component_count() as usize,
                )
                .map_err(|_| "unknown row")
        }
        fn attribute_range(&self, _: u32, _: u32) -> Result<Option<[f64; 2]>, Self::Error> {
            panic!("render loading must use the range already selected by the caller")
        }
    }

    #[test]
    fn modes_load_only_required_payloads_with_alternate_and_dynamic_reader() {
        for (mode, calls, columns) in [
            (ColorMode::Rgb, vec!["xyz", "rgb"], 0),
            (ColorMode::Elevation, vec!["xyz"], 0),
            (ColorMode::Intensity, vec!["xyz", "intensity"], 1),
            (ColorMode::Classification, vec!["xyz", "classification"], 1),
        ] {
            let mut reader = MemoryReader::new();
            let source: &mut dyn OctreeReader<Error = &'static str> = &mut reader;
            let block = load_render_block(source, BlockId::new(7), Some(mode), None, None).unwrap();
            assert_eq!(reader.calls, calls);
            assert_eq!(block.id(), BlockId::new(7));
            assert_eq!(block.attributes().len(), columns);
            assert_eq!(
                block.points().points()[0].color,
                if mode == ColorMode::Rgb {
                    ColorRgb::new(10, 20, 30)
                } else {
                    ColorRgb::new(160, 160, 160)
                }
            );
        }
    }

    #[test]
    fn selected_component_uses_positions_and_its_column_without_rgb() {
        let mut reader = MemoryReader::new();
        let block = load_render_block(
            &mut reader,
            BlockId::new(7),
            Some(ColorMode::Rgb),
            Some((2, 1)),
            Some([-1.0, 1.0]),
        )
        .unwrap();
        assert_eq!(reader.calls, ["xyz", "vector"]);
        assert_eq!(block.points().points()[0].color, ColorRgb::new(0, 0, 255));
        assert_eq!(block.points().points()[1].color, ColorRgb::new(255, 0, 0));
        assert!(block.attributes().is_empty());
    }

    #[test]
    fn missing_companion_keeps_gray_and_errors_stop_loading() {
        let mut reader = MemoryReader::new();
        reader.attributes_available = false;
        let block = load_render_block(
            &mut reader,
            BlockId::new(7),
            Some(ColorMode::Rgb),
            None,
            None,
        )
        .unwrap();
        assert_eq!(reader.calls, ["xyz"]);
        assert_eq!(
            block.points().points()[0].color,
            ColorRgb::new(160, 160, 160)
        );
        let block = load_render_block(
            &mut reader,
            BlockId::new(7),
            Some(ColorMode::Intensity),
            None,
            None,
        )
        .unwrap();
        assert!(block.attributes().is_empty());
        for (failure, message) in [("xyz", "positions failed"), ("column", "column failed")] {
            let mut reader = MemoryReader::new();
            reader.fail = Some(failure);
            assert_eq!(
                load_render_block(
                    &mut reader,
                    BlockId::new(7),
                    Some(ColorMode::Intensity),
                    None,
                    None
                )
                .unwrap_err(),
                message
            );
            assert_eq!(reader.calls, ["xyz"]);
        }
    }

    #[test]
    fn cell_blocks_read_intensity_and_class_for_three_decoded_bytes_per_point() {
        let mut reader = MemoryReader::new();
        let block = load_cell_block(&mut reader, BlockId::new(7), None).unwrap();
        assert_eq!(reader.calls, ["xyz", "rgb", "intensity", "classification"]);
        let names: Vec<_> = block
            .attributes()
            .iter()
            .map(|column| column.descriptor().name())
            .collect();
        assert_eq!(names, ["intensity", "classification"]);
        assert_eq!(block.points().points()[0].color, ColorRgb::new(10, 20, 30));
        // Extra decoded bytes per point over the RGB read: the two columns' payload.
        let extra: usize = block
            .attributes()
            .iter()
            .map(|column| match column.values() {
                AttributeValues::U16(values) => values.len() * 2,
                AttributeValues::U8(values) => values.len(),
                _ => unreachable!("fixture schema"),
            })
            .sum();
        assert_eq!(extra, block.points().points().len() * 3);
        // Without a companion file the cells keep RGB and have no columns.
        let mut reader = MemoryReader::new();
        reader.attributes_available = false;
        let block = load_cell_block(&mut reader, BlockId::new(7), None).unwrap();
        assert!(block.attributes().is_empty());
    }

    fn packed(block: PointBlock) -> Vec<(u16, i16)> {
        use viewer_core::SpatialBounds;
        use viewer_render_wgpu::{
            CellGrid, NodeCellSource, NodeInput, PointNodeCellSource, pack_node,
        };
        let grid = CellGrid {
            root_min: [0.0; 3],
            root_side: 1.0,
        };
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let id = NodeId::new(0);
        let cells = PointNodeCellSource::default()
            .node_cells(&grid, id, bounds, None, true, &[block])
            .unwrap();
        let node = pack_node(
            &grid,
            [0.0; 3],
            NodeInput {
                id,
                bounds,
                points: 2,
                local_grid: grid,
                cells: &cells,
            },
        )
        .unwrap();
        node.levels
            .iter()
            .flat_map(|level| level.instances.iter())
            .filter(|cell| !cell.is_empty())
            .map(|cell| (cell.intensity(), cell.class()))
            .collect::<Vec<_>>()
    }

    #[test]
    fn cells_built_in_rgb_mode_carry_intensity_and_class_words() {
        let mut reader = MemoryReader::new();
        let with_columns = packed(load_cell_block(&mut reader, BlockId::new(7), None).unwrap());
        assert!(!with_columns.is_empty());
        assert!(with_columns.iter().all(|word| *word == (150, 2)));
        // What the CPU path's RGB read gave the cells before: no intensity, no class.
        let mut reader = MemoryReader::new();
        let rgb = load_render_block(
            &mut reader,
            BlockId::new(7),
            Some(ColorMode::Rgb),
            None,
            None,
        )
        .unwrap();
        assert!(packed(rgb).iter().all(|word| *word == (u16::MAX, -1)));
    }

    fn carried_columns(
        reader: &mut MemoryReader,
        schema: &[AttributeDescriptor],
    ) -> Vec<AttributeColumn> {
        ["intensity", "classification"]
            .iter()
            .map(|name| {
                let index = schema.iter().position(|d| d.name() == *name).unwrap();
                reader
                    .read_attribute(BlockId::new(7), index as u32)
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn async_cell_blocks_carry_intensity_and_class_in_rgb_mode() {
        // The block the build worker's async path delivers in RGB mode: geometry plus the
        // intensity and classification columns it plans with `geometryAttributes` (R3-Q32).
        let mut reader = MemoryReader::new();
        let geometry = reader.read_geometry(BlockId::new(7)).unwrap();
        let schema = reader.metadata().hierarchy().attributes().to_vec();
        let carried = carried_columns(&mut reader, &schema);
        let block = crate::lod_driver::assemble_async_block(
            BlockId::new(7),
            geometry,
            None,
            carried,
            &schema,
            None,
            None,
            usize::MAX,
        )
        .unwrap();
        let words = packed(block);
        assert!(!words.is_empty());
        assert!(words.iter().all(|word| *word == (150, 2)), "{words:?}");
    }

    #[test]
    fn async_cell_blocks_painted_with_intensity_still_carry_its_words() {
        let mut reader = MemoryReader::new();
        let geometry = reader.read_geometry(BlockId::new(7)).unwrap();
        let schema = reader.metadata().hierarchy().attributes().to_vec();
        let painted = reader.read_attribute(BlockId::new(7), 0).unwrap();
        // Carried holds only the columns other than the painted one (the worker drops it).
        let carried = vec![reader.read_attribute(BlockId::new(7), 1).unwrap()];
        let block = crate::lod_driver::assemble_async_block(
            BlockId::new(7),
            geometry,
            Some(painted),
            carried,
            &schema,
            Some((0, 0)),
            Some([0.0, 255.0]),
            usize::MAX,
        )
        .unwrap();
        let words = packed(block);
        assert!(!words.is_empty());
        assert!(words.iter().all(|word| *word == (150, 2)), "{words:?}");
    }
}
