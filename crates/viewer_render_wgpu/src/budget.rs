use crate::GpuRendererError;

/// Limits for instance data owned by one renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBudget {
    /// Maximum capacity of scene and block buffers, including an atomic update's temporary peak.
    pub max_bytes: u64,
    /// Maximum number of visible instances, independent of buffer capacity.
    pub max_visible_instances: usize,
}

impl Default for GpuBudget {
    fn default() -> Self {
        Self {
            max_bytes: 256 * 1024 * 1024,
            max_visible_instances: 4_000_000,
        }
    }
}

pub(crate) fn buffer_capacity(
    bytes: u64,
    minimum_size: u64,
    device_limit: u64,
) -> Result<u64, GpuRendererError> {
    let required = bytes.max(minimum_size);
    let capacity = required
        .checked_next_power_of_two()
        .unwrap_or(device_limit)
        .min(device_limit);
    if capacity < required {
        return Err(GpuRendererError::GpuBufferLimitExceeded {
            requested_bytes: required,
            max_bytes: device_limit,
        });
    }
    Ok(capacity)
}

pub(crate) fn check_visible_instances(
    requested: usize,
    limit: usize,
) -> Result<(), GpuRendererError> {
    if requested > limit {
        return Err(GpuRendererError::VisibleInstanceLimitExceeded {
            requested_instances: requested,
            max_instances: limit,
        });
    }
    Ok(())
}

pub(crate) fn check_peak_bytes(
    current_bytes: u64,
    added_capacity: u64,
    limit: u64,
) -> Result<(), GpuRendererError> {
    let required_bytes = current_bytes
        .checked_add(added_capacity)
        .ok_or(GpuRendererError::InvalidSceneSize)?;
    if required_bytes > limit {
        return Err(GpuRendererError::GpuBudgetExceeded {
            required_bytes,
            max_bytes: limit,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{buffer_capacity, check_peak_bytes, check_visible_instances};
    use crate::GpuRendererError;

    #[test]
    fn capacity_rounds_up_and_obeys_device_limit() {
        assert_eq!(buffer_capacity(41, 40, 128), Ok(64));
        assert_eq!(buffer_capacity(64, 40, 64), Ok(64));
        assert_eq!(
            buffer_capacity(65, 40, 64),
            Err(GpuRendererError::GpuBufferLimitExceeded {
                requested_bytes: 65,
                max_bytes: 64,
            })
        );
    }

    #[test]
    fn peak_budget_includes_old_and_new_allocations() {
        assert_eq!(check_peak_bytes(64, 64, 128), Ok(()));
        assert_eq!(
            check_peak_bytes(64, 65, 128),
            Err(GpuRendererError::GpuBudgetExceeded {
                required_bytes: 129,
                max_bytes: 128,
            })
        );
    }

    #[test]
    fn visible_instance_limit_is_strict() {
        assert_eq!(check_visible_instances(10, 10), Ok(()));
        assert_eq!(
            check_visible_instances(11, 10),
            Err(GpuRendererError::VisibleInstanceLimitExceeded {
                requested_instances: 11,
                max_instances: 10,
            })
        );
    }
}
