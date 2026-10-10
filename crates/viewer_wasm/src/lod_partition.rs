#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use viewer_core::{AttributeColumn, AttributeValues, SpatialSubdivision};
use viewer_format::SourcePointBlock;

pub(crate) fn partition_block(
    block: &SourcePointBlock,
    world_positions: &[[f64; 3]],
    subdivision: SpatialSubdivision,
) -> Result<Vec<(usize, SourcePointBlock)>, String> {
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
            let attributes = block
                .attributes()
                .iter()
                .map(|column| {
                    let component_count = column.descriptor().component_count() as usize;
                    let values =
                        select_attribute_values(column.values(), &point_indices, component_count)?;
                    AttributeColumn::new(column.descriptor().clone(), values, point_indices.len())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                cell,
                SourcePointBlock::new(viewer_core::BlockId::new(0), positions, colors, attributes),
            ))
        })
        .collect()
}

fn select_attribute_values(
    source: &AttributeValues,
    indices: &[usize],
    components: usize,
) -> Result<AttributeValues, String> {
    macro_rules! select_values {
        ($values:expr, $variant:ident) => {{
            let values = $values;
            let mut selected = Vec::with_capacity(indices.len().saturating_mul(components));
            for index in indices {
                let start = index
                    .checked_mul(components)
                    .ok_or("spatial attribute index overflow")?;
                let end = start
                    .checked_add(components)
                    .ok_or("spatial attribute index overflow")?;
                selected.extend_from_slice(
                    values
                        .get(start..end)
                        .ok_or("spatial attribute is shorter than its point block")?,
                );
            }
            Ok(AttributeValues::$variant(selected))
        }};
    }
    match source {
        AttributeValues::U8(values) => select_values!(values, U8),
        AttributeValues::I8(values) => select_values!(values, I8),
        AttributeValues::U16(values) => select_values!(values, U16),
        AttributeValues::I16(values) => select_values!(values, I16),
        AttributeValues::U32(values) => select_values!(values, U32),
        AttributeValues::I32(values) => select_values!(values, I32),
        AttributeValues::U64(values) => select_values!(values, U64),
        AttributeValues::I64(values) => select_values!(values, I64),
        AttributeValues::F32(values) => select_values!(values, F32),
        AttributeValues::F64(values) => select_values!(values, F64),
    }
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
        for (cell, partition) in partitions {
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
