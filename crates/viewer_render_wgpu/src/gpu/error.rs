//! Errors of the surface renderer.

use viewer_core::BlockId;

use crate::CellPoolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuRendererError {
    NoCompatibleAdapter,
    Offscreen(String),
    UnsupportedSurface,
    DeviceRequest(String),
    InvalidSceneSize,
    InvalidSceneData,
    InvalidVisualScale,
    InvalidGpuBudget,
    GpuBufferLimitExceeded {
        requested_bytes: u64,
        max_bytes: u64,
    },
    GpuVertexStrideLimitExceeded {
        required_bytes: u64,
        max_bytes: u64,
    },
    VisibleInstanceLimitExceeded {
        requested_instances: usize,
        max_instances: usize,
    },
    GpuBudgetExceeded {
        required_bytes: u64,
        max_bytes: u64,
    },
    InvalidBlockData(BlockId),
    MissingBlockPayload(BlockId),
    PipelineValidation(String),
    SurfaceAcquisition(String),
    CellPool(CellPoolError),
    /// The `PageFrame` of the page entries uses another coordinate origin or visual scale.
    PageFrameMismatch,
}

impl std::fmt::Display for GpuRendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offscreen(error) => write!(f, "offscreen rendering failed: {error}"),
            Self::NoCompatibleAdapter => f.write_str("no GPU adapter supports this surface"),
            Self::UnsupportedSurface => f.write_str("surface has no usable presentation format"),
            Self::DeviceRequest(error) => write!(f, "GPU device request failed: {error}"),
            Self::InvalidSceneSize => f.write_str("scene exceeds supported GPU buffer limits"),
            Self::InvalidSceneData => f.write_str("scene contains invalid render data"),
            Self::InvalidVisualScale => {
                f.write_str("visual scale must be positive, finite, and representable as f32")
            }
            Self::InvalidGpuBudget => f.write_str(
                "GPU budget must allow at least one instance and a nonzero instance count",
            ),
            Self::GpuBufferLimitExceeded {
                requested_bytes,
                max_bytes,
            } => write!(
                f,
                "GPU buffer requires {requested_bytes} bytes, device limit is {max_bytes}"
            ),
            Self::GpuVertexStrideLimitExceeded {
                required_bytes,
                max_bytes,
            } => write!(
                f,
                "instance vertex stride requires {required_bytes} bytes, device limit is {max_bytes}"
            ),
            Self::VisibleInstanceLimitExceeded {
                requested_instances,
                max_instances,
            } => write!(
                f,
                "visible selection requires {requested_instances} instances, configured limit is {max_instances}"
            ),
            Self::GpuBudgetExceeded {
                required_bytes,
                max_bytes,
            } => write!(
                f,
                "atomic GPU buffer update requires a peak of {required_bytes} bytes, configured budget is {max_bytes}"
            ),
            Self::InvalidBlockData(id) => write!(f, "block {id:?} contains invalid render data"),
            Self::MissingBlockPayload(id) => {
                write!(f, "block {id:?} is not resident in the CPU cache")
            }
            Self::PipelineValidation(error) => write!(f, "GPU pipeline is invalid: {error}"),
            Self::SurfaceAcquisition(error) => write!(f, "GPU surface acquisition failed: {error}"),
            Self::CellPool(error) => write!(f, "cell pool: {error}"),
            Self::PageFrameMismatch => f.write_str(
                "page frame does not match the renderer's coordinate origin and visual scale",
            ),
        }
    }
}

impl std::error::Error for GpuRendererError {}
