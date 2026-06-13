use crate::{Aabb, ColorRgb, SparseVoxelStore};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleVoxel {
    pub center: [f32; 3],
    pub size: f32,
    pub color: ColorRgb,
    pub point_count: u32,
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
        let mut visible = Vec::new();

        for voxel in self.iter_voxels() {
            if let Some(bounds) = query.bounds {
                if !bounds.contains_point(voxel.center) {
                    continue;
                }
            }

            visible.push(VisibleVoxel {
                center: voxel.center,
                size: voxel.size,
                color: voxel.color,
                point_count: voxel.point_count,
            });

            if let Some(limit) = query.limit {
                if visible.len() >= limit {
                    break;
                }
            }
        }

        VisibleVoxelBatch::new(visible)
    }
}
