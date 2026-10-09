use crate::{Aabb, ColorRgb, SparseVoxelStore};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleVoxel {
    pub center: [f32; 3],
    pub size: f32,
    pub color: ColorRgb,
    pub point_count: u64,
}

impl VisibleVoxel {
    pub fn color_linear_rgba(self) -> [f32; 4] {
        self.color.to_linear_rgba()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisibleVoxelBatch {
    voxels: Vec<VisibleVoxel>,
}

impl VisibleVoxelBatch {
    pub fn new(voxels: Vec<VisibleVoxel>) -> Self {
        Self { voxels }
    }

    pub fn len(&self) -> usize {
        self.voxels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.voxels.is_empty()
    }

    pub fn voxels(&self) -> &[VisibleVoxel] {
        &self.voxels
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VisibilityQuery {
    pub bounds: Option<Aabb>,
    pub limit: Option<usize>,
}

impl SparseVoxelStore {
    pub fn build_visible_batch(&self, query: VisibilityQuery) -> VisibleVoxelBatch {
        if query.limit == Some(0) {
            return VisibleVoxelBatch::new(Vec::new());
        }

        let visible = self
            .iter_voxels()
            .filter(|voxel| {
                query
                    .bounds
                    .is_none_or(|bounds| bounds.contains_point(voxel.center))
            })
            .map(|voxel| VisibleVoxel {
                center: voxel.center,
                size: voxel.size,
                color: voxel.color,
                point_count: voxel.point_count,
            });

        if let Some(limit) = query.limit {
            let mut visible = visible.enumerate().collect::<Vec<_>>();
            if limit < visible.len() {
                visible.select_nth_unstable_by(limit, compare_indexed_visible);
                visible.truncate(limit);
            }
            visible.sort_unstable_by(compare_indexed_visible);
            VisibleVoxelBatch::new(visible.into_iter().map(|(_, voxel)| voxel).collect())
        } else {
            let mut visible = visible.collect::<Vec<_>>();
            visible.sort_by(compare_visible);
            VisibleVoxelBatch::new(visible)
        }
    }
}

fn compare_visible(left: &VisibleVoxel, right: &VisibleVoxel) -> std::cmp::Ordering {
    left.center[0]
        .total_cmp(&right.center[0])
        .then_with(|| left.center[1].total_cmp(&right.center[1]))
        .then_with(|| left.center[2].total_cmp(&right.center[2]))
}

fn compare_indexed_visible(
    (left_index, left): &(usize, VisibleVoxel),
    (right_index, right): &(usize, VisibleVoxel),
) -> std::cmp::Ordering {
    compare_visible(left, right).then_with(|| left_index.cmp(right_index))
}
