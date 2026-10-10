use viewer_core::{AttributeColumn, AttributeDescriptor, AttributeValues, BlockId, ColorRgb};
use viewer_format::SourcePointBlock;

pub(crate) const MAX_REPRESENTATIVE_POINTS: u64 = 4_096;

pub(crate) struct RepresentativeBuffer {
    schema: Vec<AttributeDescriptor>,
    positions: Vec<[f64; 3]>,
    colors: Vec<ColorRgb>,
    attributes: Vec<AttributeValues>,
    seen_points: u64,
    point_limit: u64,
}

impl RepresentativeBuffer {
    #[cfg(test)]
    pub(crate) fn new(schema: Vec<AttributeDescriptor>) -> Self {
        Self::with_limit(schema, MAX_REPRESENTATIVE_POINTS)
    }

    pub(crate) fn with_limit(schema: Vec<AttributeDescriptor>, point_limit: u64) -> Self {
        let attributes = schema
            .iter()
            .map(|descriptor| match descriptor.data_type() {
                viewer_core::AttributeDataType::U8 => AttributeValues::U8(Vec::new()),
                viewer_core::AttributeDataType::I8 => AttributeValues::I8(Vec::new()),
                viewer_core::AttributeDataType::U16 => AttributeValues::U16(Vec::new()),
                viewer_core::AttributeDataType::I16 => AttributeValues::I16(Vec::new()),
                viewer_core::AttributeDataType::U32 => AttributeValues::U32(Vec::new()),
                viewer_core::AttributeDataType::I32 => AttributeValues::I32(Vec::new()),
                viewer_core::AttributeDataType::U64 => AttributeValues::U64(Vec::new()),
                viewer_core::AttributeDataType::I64 => AttributeValues::I64(Vec::new()),
                viewer_core::AttributeDataType::F32 => AttributeValues::F32(Vec::new()),
                viewer_core::AttributeDataType::F64 => AttributeValues::F64(Vec::new()),
            })
            .collect();
        Self {
            schema,
            positions: Vec::new(),
            colors: Vec::new(),
            attributes,
            seen_points: 0,
            point_limit,
        }
    }

    pub(crate) fn include_block(&mut self, block: &SourcePointBlock) -> Result<(), String> {
        let component_counts = self
            .schema
            .iter()
            .map(|descriptor| descriptor.component_count() as usize)
            .collect::<Vec<_>>();
        for index in 0..block.positions_local().len() {
            let sample_index = if self.seen_points < self.point_limit {
                Some(self.seen_points as usize)
            } else {
                let candidate = stable_sample_hash(self.seen_points) % (self.seen_points + 1);
                (candidate < self.point_limit).then_some(candidate as usize)
            };
            self.seen_points = self.seen_points.saturating_add(1);
            let Some(sample_index) = sample_index else {
                continue;
            };
            let append = sample_index == self.positions.len();
            if append {
                self.positions.push(block.positions_local()[index]);
                self.colors.push(block.colors()[index]);
            } else {
                self.positions[sample_index] = block.positions_local()[index];
                self.colors[sample_index] = block.colors()[index];
            }
            for ((target, column), component_count) in self
                .attributes
                .iter_mut()
                .zip(block.attributes())
                .zip(&component_counts)
            {
                set_attribute_sample(
                    target,
                    column.values(),
                    sample_index,
                    index,
                    *component_count,
                    append,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn point_count(&self) -> usize {
        self.positions.len()
    }

    pub(crate) fn into_block(self, id: BlockId) -> Result<SourcePointBlock, String> {
        let count = self.positions.len();
        let attributes = self
            .schema
            .into_iter()
            .zip(self.attributes)
            .map(|(descriptor, values)| {
                AttributeColumn::new(descriptor, values, count).map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SourcePointBlock::new(
            id,
            self.positions,
            self.colors,
            attributes,
        ))
    }
}

fn set_attribute_sample(
    target: &mut AttributeValues,
    source: &AttributeValues,
    sample_index: usize,
    point_index: usize,
    components: usize,
    append: bool,
) -> Result<(), String> {
    let source_start = point_index
        .checked_mul(components)
        .ok_or("attribute sample index overflow")?;
    let source_end = source_start
        .checked_add(components)
        .ok_or("attribute sample index overflow")?;
    let target_start = sample_index
        .checked_mul(components)
        .ok_or("attribute sample index overflow")?;
    let target_end = target_start
        .checked_add(components)
        .ok_or("attribute sample index overflow")?;
    macro_rules! set_values {
        ($target:expr, $source:expr) => {{
            let values = $source
                .get(source_start..source_end)
                .ok_or("attribute sample is out of range")?;
            if append {
                $target.extend_from_slice(values);
            } else {
                let destination = $target
                    .get_mut(target_start..target_end)
                    .ok_or("attribute sample slot is out of range")?;
                destination.copy_from_slice(values);
            }
        }};
    }
    match (target, source) {
        (AttributeValues::U8(target), AttributeValues::U8(source)) => set_values!(target, source),
        (AttributeValues::I8(target), AttributeValues::I8(source)) => set_values!(target, source),
        (AttributeValues::U16(target), AttributeValues::U16(source)) => set_values!(target, source),
        (AttributeValues::I16(target), AttributeValues::I16(source)) => set_values!(target, source),
        (AttributeValues::U32(target), AttributeValues::U32(source)) => set_values!(target, source),
        (AttributeValues::I32(target), AttributeValues::I32(source)) => set_values!(target, source),
        (AttributeValues::U64(target), AttributeValues::U64(source)) => set_values!(target, source),
        (AttributeValues::I64(target), AttributeValues::I64(source)) => set_values!(target, source),
        (AttributeValues::F32(target), AttributeValues::F32(source)) => set_values!(target, source),
        (AttributeValues::F64(target), AttributeValues::F64(source)) => set_values!(target, source),
        _ => return Err("attribute sample type differs from output schema".into()),
    }
    Ok(())
}

fn stable_sample_hash(value: u64) -> u64 {
    let mut value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::{MAX_REPRESENTATIVE_POINTS, RepresentativeBuffer};
    use viewer_core::{AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues};
    use viewer_core::{BlockId, ColorRgb};
    use viewer_format::SourcePointBlock;

    fn source_block(count: usize) -> SourcePointBlock {
        let descriptor =
            AttributeDescriptor::new("source_index", AttributeDataType::U32, 1).unwrap();
        let positions = (0..count).map(|index| [index as f64, 0.0, 0.0]).collect();
        let colors = (0..count)
            .map(|index| ColorRgb::new(index as u8, 0, 0))
            .collect();
        let indices = (0..count).map(|index| index as u32).collect();
        let attribute =
            AttributeColumn::new(descriptor, AttributeValues::U32(indices), count).unwrap();
        SourcePointBlock::new(BlockId::new(0), positions, colors, vec![attribute])
    }

    #[test]
    fn samples_are_bounded_deterministic_and_keep_attributes_attached() {
        let source = source_block(8_192);
        let descriptor = source.attributes()[0].descriptor().clone();
        let mut first = RepresentativeBuffer::new(vec![descriptor.clone()]);
        let mut second = RepresentativeBuffer::new(vec![descriptor]);
        first.include_block(&source).unwrap();
        second.include_block(&source).unwrap();
        assert_eq!(first.point_count(), MAX_REPRESENTATIVE_POINTS as usize);
        assert_eq!(second.point_count(), MAX_REPRESENTATIVE_POINTS as usize);

        let first = first.into_block(BlockId::new(1)).unwrap();
        let second = second.into_block(BlockId::new(1)).unwrap();
        assert_eq!(
            first.positions_local().len(),
            MAX_REPRESENTATIVE_POINTS as usize
        );
        assert_eq!(first, second);
        let AttributeValues::U32(indices) = first.attributes()[0].values() else {
            panic!("source index attribute changed type");
        };
        for (position, index) in first.positions_local().iter().zip(indices) {
            assert_eq!(position[0] as u32, *index);
        }
    }

    #[test]
    fn keeps_all_points_when_source_is_smaller_than_the_sample_limit() {
        let source = source_block(12);
        let descriptor = source.attributes()[0].descriptor().clone();
        let mut representatives = RepresentativeBuffer::new(vec![descriptor]);
        representatives.include_block(&source).unwrap();
        assert_eq!(representatives.point_count(), 12);
        let samples = representatives.into_block(BlockId::new(1)).unwrap();
        assert_eq!(samples.positions_local(), source.positions_local());
        assert_eq!(samples.attributes(), source.attributes());
    }
}
