use viewer_core::{
    AttributeColumn, AttributeDescriptor, AttributeValues, BlockId, ColorRgb, SpatialBounds,
    SpatialSubdivision,
};
use viewer_format::SourcePointBlock;

#[cfg(target_arch = "wasm32")]
pub(crate) const MAX_REPRESENTATIVE_POINTS: u64 = 4_096;
const REPRESENTATIVE_VOXEL_DIVISIONS: u8 = 8;

pub(crate) struct VoxelRepresentativeBuffer {
    schema: Vec<AttributeDescriptor>,
    subdivision: SpatialSubdivision,
    origin: [f64; 3],
    voxels: Vec<Option<VoxelAccumulator>>,
}

impl VoxelRepresentativeBuffer {
    pub(crate) fn new(
        schema: Vec<AttributeDescriptor>,
        bounds: SpatialBounds,
        origin: [f64; 3],
    ) -> Result<Self, String> {
        let subdivision = SpatialSubdivision::new(bounds, REPRESENTATIVE_VOXEL_DIVISIONS)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            schema,
            subdivision,
            origin,
            voxels: (0..subdivision.cell_count()).map(|_| None).collect(),
        })
    }

    pub(crate) fn include_block(
        &mut self,
        block: &SourcePointBlock,
        world_positions: &[[f64; 3]],
    ) -> Result<(), String> {
        if world_positions.len() != block.positions_local().len() {
            return Err("voxel positions do not match the source block".into());
        }
        for (index, position) in world_positions.iter().copied().enumerate() {
            let voxel_index = self.subdivision.cell_index(position);
            let bounds = self
                .subdivision
                .cell_bounds(voxel_index)
                .map_err(|error| error.to_string())?;
            let center = std::array::from_fn(|axis| {
                bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) / 2.0
            });
            let accumulator =
                self.voxels[voxel_index].get_or_insert_with(VoxelAccumulator::default);
            accumulator.add(block, index, position, center)?;
        }
        Ok(())
    }

    pub(crate) fn point_count(&self) -> usize {
        self.voxels.iter().filter(|voxel| voxel.is_some()).count()
    }

    pub(crate) fn into_block(self, id: BlockId) -> Result<SourcePointBlock, String> {
        let count = self.point_count();
        let mut positions = Vec::with_capacity(count);
        let mut colors = Vec::with_capacity(count);
        let mut attributes = self
            .schema
            .iter()
            .map(|descriptor| AttributeValues::empty(descriptor.data_type()))
            .collect::<Vec<_>>();
        for (index, voxel) in self.voxels.into_iter().enumerate() {
            let Some(voxel) = voxel else {
                continue;
            };
            let bounds = self
                .subdivision
                .cell_bounds(index)
                .map_err(|error| error.to_string())?;
            let center = std::array::from_fn(|axis| {
                bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) / 2.0
                    - self.origin[axis]
            });
            positions.push(center);
            colors.push(voxel.average_color());
            for ((descriptor, target), nearest) in self
                .schema
                .iter()
                .zip(&mut attributes)
                .zip(&voxel.nearest_attributes)
            {
                if descriptor.name() == "rgb" {
                    append_average_rgb(target, &voxel.rgb_sum, voxel.count)?;
                } else {
                    target
                        .extend_from(nearest)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        let attributes = self
            .schema
            .into_iter()
            .zip(attributes)
            .map(|(descriptor, values)| {
                AttributeColumn::new(descriptor, values, count).map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SourcePointBlock::new(id, positions, colors, attributes))
    }
}

#[derive(Default)]
struct VoxelAccumulator {
    count: u64,
    color_sum: [u128; 3],
    rgb_sum: [u128; 3],
    nearest_distance_squared: f64,
    nearest_attributes: Vec<AttributeValues>,
}

impl VoxelAccumulator {
    fn add(
        &mut self,
        block: &SourcePointBlock,
        index: usize,
        position: [f64; 3],
        center: [f64; 3],
    ) -> Result<(), String> {
        self.count += 1;
        let color = block.colors()[index];
        for (axis, value) in [color.r, color.g, color.b].into_iter().enumerate() {
            self.color_sum[axis] += u128::from(value);
        }
        for column in block.attributes() {
            if column.descriptor().name() == "rgb"
                && let AttributeValues::U16(values) = column.values()
            {
                let start = index.checked_mul(3).ok_or("RGB attribute index overflow")?;
                let end = start.checked_add(3).ok_or("RGB attribute index overflow")?;
                let rgb = values
                    .get(start..end)
                    .ok_or("RGB attribute is incomplete")?;
                for (axis, value) in rgb.iter().copied().enumerate() {
                    self.rgb_sum[axis] += u128::from(value);
                }
            }
        }
        let delta = [
            position[0] - center[0],
            position[1] - center[1],
            position[2] - center[2],
        ];
        let distance_squared = delta.iter().map(|value| value * value).sum::<f64>();
        if self.nearest_attributes.is_empty() || distance_squared < self.nearest_distance_squared {
            self.nearest_distance_squared = distance_squared;
            self.nearest_attributes = block
                .attributes()
                .iter()
                .map(|column| {
                    select_value(
                        column.values(),
                        index,
                        column.descriptor().component_count() as usize,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
        }
        Ok(())
    }

    fn average_color(&self) -> ColorRgb {
        ColorRgb::new(
            rounded_average(self.color_sum[0], self.count) as u8,
            rounded_average(self.color_sum[1], self.count) as u8,
            rounded_average(self.color_sum[2], self.count) as u8,
        )
    }
}

fn select_value(
    source: &AttributeValues,
    index: usize,
    components: usize,
) -> Result<AttributeValues, String> {
    source
        .select_rows(&[index], components)
        .map_err(|error| error.to_string())
}

fn append_average_rgb(
    target: &mut AttributeValues,
    sums: &[u128; 3],
    count: u64,
) -> Result<(), String> {
    let AttributeValues::U16(values) = target else {
        return Err("RGB attribute must use U16 values".into());
    };
    values.extend(sums.iter().map(|sum| rounded_average(*sum, count) as u16));
    Ok(())
}

fn rounded_average(sum: u128, count: u64) -> u128 {
    (sum + u128::from(count / 2)) / u128::from(count)
}

#[cfg(test)]
mod tests {
    use super::VoxelRepresentativeBuffer;
    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
        ColorRgb, SpatialBounds,
    };
    use viewer_format::SourcePointBlock;

    #[test]
    fn voxel_representatives_average_color_and_rgb_but_keep_nearest_point_attributes() {
        let schema = vec![
            AttributeDescriptor::new("rgb", AttributeDataType::U16, 3).unwrap(),
            AttributeDescriptor::new("classification", AttributeDataType::U8, 1).unwrap(),
        ];
        let block = SourcePointBlock::new(
            BlockId::new(0),
            vec![[0.7; 3], [1.1; 3]],
            vec![ColorRgb::new(10, 30, 50), ColorRgb::new(30, 50, 70)],
            vec![
                AttributeColumn::new(
                    schema[0].clone(),
                    AttributeValues::U16(vec![100, 100, 100, 300, 300, 300]),
                    2,
                )
                .unwrap(),
                AttributeColumn::new(schema[1].clone(), AttributeValues::U8(vec![2, 5]), 2)
                    .unwrap(),
            ],
        );
        let mut representatives = VoxelRepresentativeBuffer::new(
            schema,
            SpatialBounds::new([0.0; 3], [10.0; 3]).unwrap(),
            [0.0; 3],
        )
        .unwrap();
        representatives
            .include_block(&block, &[[0.7; 3], [1.1; 3]])
            .unwrap();

        let output = representatives.into_block(BlockId::new(1)).unwrap();
        assert_eq!(output.positions_local(), &[[0.625; 3]]);
        assert_eq!(output.colors(), &[ColorRgb::new(20, 40, 60)]);
        assert_eq!(
            output.attributes()[0].values(),
            &AttributeValues::U16(vec![200, 200, 200])
        );
        assert_eq!(
            output.attributes()[1].values(),
            &AttributeValues::U8(vec![2])
        );
    }
}
