use viewer_core::{BlockId, PointBatch, PointBlock};
use viewer_format::OctreeReader;
use viewer_render_wgpu::ColorMode;

/// Schema index of the `intensity` column, the one a block without the selected column is
/// painted with (`v8.1-pre` LOD rows keep only what the viewer draws).
pub(crate) fn intensity_index(schema: &[viewer_core::AttributeDescriptor]) -> Option<u32> {
    schema
        .iter()
        .position(|descriptor| descriptor.name() == "intensity")
        .and_then(|index| u32::try_from(index).ok())
}

/// Paints `geometry` with the selected `(column, component)` and its range. A block that omits
/// that column (a LOD block of `v8.1-pre`) is painted with intensity and intensity's global
/// range instead; when it has no intensity either, the block keeps the display colours it has.
fn paint_selected_attribute<E: std::fmt::Display>(
    reader: &mut dyn OctreeReader<Error = E>,
    id: BlockId,
    geometry: PointBatch,
    (index, component): (u32, u32),
    range: Option<[f64; 2]>,
) -> Result<PointBatch, String> {
    let (index, component, range) = if reader
        .attribute_present(id, index)
        .map_err(|e| e.to_string())?
    {
        (index, component, range)
    } else {
        let Some(intensity) = intensity_index(reader.metadata().hierarchy().attributes()) else {
            return Ok(geometry);
        };
        if !reader
            .attribute_present(id, intensity)
            .map_err(|e| e.to_string())?
        {
            return Ok(geometry);
        }
        let range = reader
            .attribute_range(intensity, 0)
            .map_err(|e| e.to_string())?;
        (intensity, 0, range)
    };
    let column = reader
        .read_attribute(id, index)
        .map_err(|e| e.to_string())?;
    viewer_render_wgpu::colorize_attribute(geometry, &column, component, range)
        .map_err(str::to_owned)
}

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
    if let Some(selected) = selected_attribute {
        geometry = paint_selected_attribute(reader, id, geometry, selected, selected_range)?;
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
        if reader
            .attribute_present(id, index)
            .map_err(|e| e.to_string())?
        {
            attributes.push(
                reader
                    .read_attribute(id, index)
                    .map_err(|e| e.to_string())?,
            );
        }
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
    if let Some((selected, range)) = paint {
        geometry = paint_selected_attribute(reader, id, geometry, selected, range)?;
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
            if reader
                .attribute_present(id, index)
                .map_err(|e| e.to_string())?
            {
                attributes.push(
                    reader
                        .read_attribute(id, index)
                        .map_err(|e| e.to_string())?,
                );
            }
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
        /// Columns the block omits (`attribute_present` is false for them).
        absent: Vec<u32>,
        present_error: bool,
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
                absent: vec![],
                present_error: false,
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
        fn attribute_present(&self, _: BlockId, index: u32) -> Result<bool, Self::Error> {
            if self.present_error {
                return Err("index failed");
            }
            Ok(!self.absent.contains(&index))
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
        fn attribute_range(
            &self,
            index: u32,
            component: u32,
        ) -> Result<Option<[f64; 2]>, Self::Error> {
            // Only the fallback to intensity asks for a range of its own (its global one).
            assert_eq!(
                (index, component),
                (0, 0),
                "render loading must use the range already selected by the caller"
            );
            Ok(Some([0.0, 200.0]))
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
            Some(carried),
            &schema,
            None,
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
            Some(carried),
            &schema,
            Some((0, 0)),
            Some([0.0, 255.0]),
            None,
            usize::MAX,
        )
        .unwrap();
        let words = packed(block);
        assert!(!words.is_empty());
        assert!(words.iter().all(|word| *word == (150, 2)), "{words:?}");
    }

    #[test]
    fn cpu_path_blocks_painted_with_an_attribute_keep_no_extra_column() {
        let mut reader = MemoryReader::new();
        let geometry = reader.read_geometry(BlockId::new(7)).unwrap();
        let schema = reader.metadata().hierarchy().attributes().to_vec();
        let painted = reader.read_attribute(BlockId::new(7), 0).unwrap();
        let block = crate::lod_driver::assemble_async_block(
            BlockId::new(7),
            geometry,
            Some(painted),
            None,
            &schema,
            Some((0, 0)),
            Some([0.0, 255.0]),
            None,
            usize::MAX,
        )
        .unwrap();
        assert!(block.attributes().is_empty());
    }

    /// The colours intensity `[100, 200]` gets over its global range `[0, 200]` (the selected
    /// column's own range `[-1, 1]` would saturate both points).
    const INTENSITY_COLOURS: [ColorRgb; 2] =
        [ColorRgb::new(127, 255, 127), ColorRgb::new(255, 0, 0)];

    #[test]
    fn attribute_mode_falls_back_to_intensity_on_lod_blocks() {
        let id = BlockId::new(7);
        // CPU path: the block lacks the selected column, so it requests intensity instead.
        let mut reader = MemoryReader::new();
        reader.absent = vec![2];
        let block = load_render_block(
            &mut reader,
            id,
            Some(ColorMode::Rgb),
            Some((2, 1)),
            Some([-1.0, 1.0]),
        )
        .unwrap();
        assert_eq!(reader.calls, ["xyz", "intensity"]);
        assert!(block.attributes().is_empty());
        let colours: Vec<_> = block.points().points().iter().map(|p| p.color).collect();
        assert_eq!(colours, INTENSITY_COLOURS);
        // Cells path: painted from intensity, and intensity and class still travel with the block.
        let mut reader = MemoryReader::new();
        reader.absent = vec![2];
        let block = load_cell_block(&mut reader, id, Some(((2, 1), Some([-1.0, 1.0])))).unwrap();
        let colours: Vec<_> = block.points().points().iter().map(|p| p.color).collect();
        assert_eq!(colours, INTENSITY_COLOURS);
        let names: Vec<_> = block
            .attributes()
            .iter()
            .map(|column| column.descriptor().name())
            .collect();
        assert_eq!(names, ["intensity", "classification"]);
        // A block that has the column paints with it, as before.
        let mut reader = MemoryReader::new();
        let block = load_render_block(
            &mut reader,
            id,
            Some(ColorMode::Rgb),
            Some((2, 1)),
            Some([-1.0, 1.0]),
        )
        .unwrap();
        assert_eq!(reader.calls, ["xyz", "vector"]);
        assert_eq!(block.points().points()[0].color, ColorRgb::new(0, 0, 255));
    }

    #[test]
    fn async_attribute_blocks_fall_back_to_intensity_with_its_global_range() {
        let mut reader = MemoryReader::new();
        let geometry = reader.read_positions(BlockId::new(7)).unwrap();
        let schema = reader.metadata().hierarchy().attributes().to_vec();
        let intensity = reader.read_attribute(BlockId::new(7), 0).unwrap();
        // Selected `vector:1` is absent: the plan delivered intensity, painted with its own range.
        let block = crate::lod_driver::assemble_async_block(
            BlockId::new(7),
            geometry,
            Some(intensity),
            Some(vec![]),
            &schema,
            Some((2, 1)),
            Some([-1.0, 1.0]),
            Some([0.0, 200.0]),
            usize::MAX,
        )
        .unwrap();
        let colours: Vec<_> = block.points().points().iter().map(|p| p.color).collect();
        assert_eq!(colours, INTENSITY_COLOURS);
        assert_eq!(block.attributes().len(), 1);
        // Neither the selected column nor intensity arrived: the block keeps its colours.
        let geometry = reader.read_geometry(BlockId::new(7)).unwrap();
        let block = crate::lod_driver::assemble_async_block(
            BlockId::new(7),
            geometry,
            None,
            None,
            &schema,
            Some((2, 1)),
            Some([-1.0, 1.0]),
            Some([0.0, 200.0]),
            usize::MAX,
        )
        .unwrap();
        assert_eq!(block.points().points()[0].color, ColorRgb::new(10, 20, 30));
        // Any other column in place of the selected one is still a mismatch.
        let geometry = reader.read_positions(BlockId::new(7)).unwrap();
        let classification = reader.read_attribute(BlockId::new(7), 1).unwrap();
        assert!(
            crate::lod_driver::assemble_async_block(
                BlockId::new(7),
                geometry,
                Some(classification),
                None,
                &schema,
                Some((2, 1)),
                Some([-1.0, 1.0]),
                Some([0.0, 200.0]),
                usize::MAX,
            )
            .is_err()
        );
    }

    #[test]
    fn absent_columns_are_skipped_and_lookup_errors_propagate() {
        let id = BlockId::new(7);
        // Paint column and mode column both omitted: own colours, no column, no error.
        let mut reader = MemoryReader::new();
        reader.absent = vec![0, 1, 2];
        let block = load_render_block(
            &mut reader,
            id,
            Some(ColorMode::Intensity),
            Some((2, 1)),
            Some([-1.0, 1.0]),
        )
        .unwrap();
        assert_eq!(reader.calls, ["xyz"]);
        assert!(block.attributes().is_empty());
        assert_eq!(
            block.points().points()[0].color,
            ColorRgb::new(160, 160, 160)
        );
        // Cells: geometry with its own RGB, and no intensity/classification columns.
        let mut reader = MemoryReader::new();
        reader.absent = vec![0, 1, 2];
        let block = load_cell_block(&mut reader, id, Some(((2, 1), Some([-1.0, 1.0])))).unwrap();
        assert_eq!(reader.calls, ["xyz", "rgb"]);
        assert!(block.attributes().is_empty());
        assert_eq!(block.points().points()[0].color, ColorRgb::new(10, 20, 30));
        // Only one of the two cell columns omitted: the other still loads.
        let mut reader = MemoryReader::new();
        reader.absent = vec![0];
        let block = load_cell_block(&mut reader, id, None).unwrap();
        let names: Vec<_> = block
            .attributes()
            .iter()
            .map(|column| column.descriptor().name())
            .collect();
        assert_eq!(names, ["classification"]);
        // A failing presence lookup is an error, not a silent skip.
        let mut reader = MemoryReader::new();
        reader.present_error = true;
        assert_eq!(
            load_render_block(&mut reader, id, Some(ColorMode::Intensity), None, None).unwrap_err(),
            "index failed"
        );
        assert!(load_cell_block(&mut reader, id, None).is_err());
        // Also on the selected-attribute and paint lookups.
        let mut reader = MemoryReader::new();
        reader.present_error = true;
        assert_eq!(
            load_render_block(&mut reader, id, None, Some((2, 1)), None).unwrap_err(),
            "index failed"
        );
        let mut reader = MemoryReader::new();
        reader.present_error = true;
        assert!(load_cell_block(&mut reader, id, Some(((2, 1), None))).is_err());
    }
}
