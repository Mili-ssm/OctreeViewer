use viewer_core::{
    Aabb, ColorRgb, Point, PointBatch, SparseVoxelStore, VisibilityQuery, VoxelCoord,
    VoxelizationConfig, VoxelizationError,
};

fn point(position: [f32; 3]) -> Point {
    Point::new(position, ColorRgb::new(10, 20, 30))
}

#[test]
fn voxel_coordinates_use_floor_for_negative_positions() {
    let config = VoxelizationConfig::new(1.0, [0.0; 3]).unwrap();
    let mut store = SparseVoxelStore::new(config);
    store
        .insert_batch(&vec![point([-0.01, 0.0, 1.0])].into())
        .unwrap();

    assert_eq!(store.voxels()[0].coord, VoxelCoord::new(-1, 0, 1));
}

#[test]
fn invalid_origin_is_rejected() {
    assert_eq!(
        VoxelizationConfig::new(1.0, [0.0, f32::INFINITY, 0.0]),
        Err(VoxelizationError::InvalidOrigin([0.0, f32::INFINITY, 0.0]))
    );
}

#[test]
fn invalid_batch_does_not_partially_change_store() {
    let config = VoxelizationConfig::new(1.0, [0.0; 3]).unwrap();
    let mut store = SparseVoxelStore::new(config);
    store.insert_batch(&vec![point([0.0; 3])].into()).unwrap();

    let batch: PointBatch = vec![point([1.0; 3]), point([f32::NAN, 0.0, 0.0])].into();
    assert!(matches!(
        store.insert_batch(&batch),
        Err(VoxelizationError::InvalidPointPosition(_))
    ));
    assert_eq!(store.point_count(), 1);
    assert_eq!(store.voxel_count(), 1);
}

#[test]
fn coordinates_outside_the_supported_range_are_rejected() {
    let config = VoxelizationConfig::new(0.001, [0.0; 3]).unwrap();
    let mut store = SparseVoxelStore::new(config);
    assert!(matches!(
        store.insert_batch(&vec![point([f32::MAX, 0.0, 0.0])].into()),
        Err(VoxelizationError::VoxelCoordinateOutOfRange { .. })
    ));
    assert_eq!(store.point_count(), 0);
}

#[test]
fn voxel_center_must_fit_the_render_coordinate_type() {
    let config = VoxelizationConfig::new(f32::MAX, [0.0; 3]).unwrap();
    let mut store = SparseVoxelStore::new(config);

    assert!(matches!(
        store.insert_batch(&vec![point([f32::MAX, 0.0, 0.0])].into()),
        Err(VoxelizationError::VoxelCoordinateOutOfRange { .. })
    ));
}

#[test]
fn zero_limit_returns_no_voxels_and_limits_are_stable() {
    let config = VoxelizationConfig::new(1.0, [0.0; 3]).unwrap();
    let mut store = SparseVoxelStore::new(config);
    store
        .insert_batch(
            &vec![
                point([2.0, 0.0, 0.0]),
                point([0.0; 3]),
                point([1.0, 0.0, 0.0]),
            ]
            .into(),
        )
        .unwrap();

    let none = store.build_visible_batch(VisibilityQuery {
        bounds: None,
        limit: Some(0),
    });
    assert!(none.is_empty());

    let first = store.build_visible_batch(VisibilityQuery {
        bounds: Some(Aabb::new([-1.0; 3], [4.0; 3])),
        limit: Some(1),
    });
    let second = store.build_visible_batch(VisibilityQuery {
        bounds: Some(Aabb::new([-1.0; 3], [4.0; 3])),
        limit: Some(1),
    });
    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
    assert_eq!(first.voxels()[0].center, [0.5; 3]);
}
