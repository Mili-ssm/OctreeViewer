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

        let mut visible = Vec::new();

        for voxel in self.iter_voxels() {
            if let Some(bounds) = query.bounds
                && !bounds.contains_point(voxel.center)
            {
                continue;
            }

            visible.push(VisibleVoxel {
                center: voxel.center,
                size: voxel.size,
                color: voxel.color,
                point_count: voxel.point_count,
            });
        }

        visible.sort_by(|left, right| {
            left.center[0]
                .total_cmp(&right.center[0])
                .then_with(|| left.center[1].total_cmp(&right.center[1]))
                .then_with(|| left.center[2].total_cmp(&right.center[2]))
        });
        if let Some(limit) = query.limit {
            visible.truncate(limit);
        }

        VisibleVoxelBatch::new(visible)
    }
}
