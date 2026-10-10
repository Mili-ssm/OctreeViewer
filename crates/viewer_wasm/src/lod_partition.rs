use viewer_core::{AttributeColumn, SpatialSubdivision};
use viewer_format::SourcePointBlock;

pub(crate) struct SpatialPartition {
    pub(crate) cell: usize,
    pub(crate) block: SourcePointBlock,
    pub(crate) world_positions: Vec<[f64; 3]>,
}

pub(crate) fn partition_block(
    block: &SourcePointBlock,
    world_positions: &[[f64; 3]],
    subdivision: SpatialSubdivision,
) -> Result<Vec<SpatialPartition>, String> {
    if world_positions.len() != block.positions_local().len() {
        return Err("spatial partition positions do not match the source block".into());
    }
    let mut indices = (0..subdivision.cell_count())
        .map(|_| Vec::new())
        .collect::<Vec<Vec<usize>>>();
    for (point_index, position) in world_positions.iter().copied().enumerate() {
        indices[subdivision.cell_index(position)].push(point_index);
    }

    indices
        .into_iter()
        .enumerate()
        .filter(|(_, indices)| !indices.is_empty())
        .map(|(cell, point_indices)| {
            let positions = point_indices
                .iter()
                .map(|index| block.positions_local()[*index])
                .collect();
            let colors = point_indices
                .iter()
                .map(|index| block.colors()[*index])
                .collect();
            let cell_positions = point_indices
                .iter()
                .map(|index| world_positions[*index])
                .collect();
            let attributes = block
                .attributes()
                .iter()
                .map(|column| {
                    let component_count = column.descriptor().component_count() as usize;
                    let values = column
                        .values()
                        .select_rows(&point_indices, component_count)
                        .map_err(|error| error.to_string())?;
                    AttributeColumn::new(column.descriptor().clone(), values, point_indices.len())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SpatialPartition {
                cell,
                block: SourcePointBlock::new(
                    viewer_core::BlockId::new(0),
                    positions,
                    colors,
                    attributes,
                ),
                world_positions: cell_positions,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::partition_block;
    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
        ColorRgb, SpatialBounds, SpatialSubdivision,
    };
    use viewer_format::SourcePointBlock;

    #[test]
    fn partitions_positions_colors_and_attributes_into_all_spatial_cells() {
        let positions = vec![
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 9.0],
            [1.0, 9.0, 1.0],
            [1.0, 9.0, 9.0],
            [9.0, 1.0, 1.0],
            [9.0, 1.0, 9.0],
            [9.0, 9.0, 1.0],
            [9.0, 9.0, 9.0],
        ];
        let colors = (0..positions.len())
            .map(|index| ColorRgb::new(index as u8, 0, 0))
            .collect::<Vec<_>>();
        let values = (0..positions.len()).map(|index| index as u32).collect();
        let attribute = AttributeColumn::new(
            AttributeDescriptor::new("source_index", AttributeDataType::U32, 1).unwrap(),
            AttributeValues::U32(values),
            positions.len(),
        )
        .unwrap();
        let block =
            SourcePointBlock::new(BlockId::new(0), positions.clone(), colors, vec![attribute]);
        let subdivision =
            SpatialSubdivision::new(SpatialBounds::new([0.0; 3], [10.0; 3]).unwrap(), 2).unwrap();

        let partitions = partition_block(&block, &positions, subdivision).unwrap();
        assert_eq!(partitions.len(), 8);
        for partition in partitions {
            let cell = partition.cell;
            assert_eq!(partition.world_positions, partition.block.positions_local());
            let partition = partition.block;
            assert_eq!(partition.positions_local().len(), 1);
            assert_eq!(subdivision.cell_index(partition.positions_local()[0]), cell);
            let source_index = partition.colors()[0].r;
            assert_eq!(
                partition.attributes()[0].values(),
                &AttributeValues::U32(vec![u32::from(source_index)])
            );
        }
    }
}
