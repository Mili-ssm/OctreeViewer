//! Metadata-only LOD baseline; no voxelization, I/O, GPU, or real-camera validation.
use std::{cell::Cell, cell::RefCell, hint::black_box, time::Instant};

use viewer_core::{
    BlockId, FrustumPlane, HierarchySemantics, LodSelectionConfig, NodeId, SpatialBounds,
    SpatialFrustum, SpatialHierarchy, SpatialNode,
};

const SAMPLES: usize = 9;

struct Case {
    name: String,
    nodes: Vec<SpatialNode>,
    base_points: u64,
    leaf_points: u64,
    clip: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        wide(64),
        wide(256),
        wide(1_024),
        binary(8),
        binary(10),
        chain(1_024),
    ];
    for case in cases {
        for semantics in [
            HierarchySemantics::Replacement,
            HierarchySemantics::Additive,
        ] {
            let full_budget = match semantics {
                HierarchySemantics::Replacement => case.leaf_points,
                HierarchySemantics::Additive => {
                    case.nodes.iter().map(SpatialNode::point_count).sum()
                }
            };
            let partial_budget = (case.base_points + full_budget) / 2;
            let hierarchy = SpatialHierarchy::new_with_semantics(
                case.nodes.clone(),
                Vec::new(),
                None,
                semantics,
            )?;
            let budgets = if partial_budget == full_budget {
                vec![full_budget]
            } else {
                vec![partial_budget, full_budget]
            };
            for budget in budgets {
                measure(&case, &hierarchy, semantics, budget, false)?;
                if case.clip {
                    measure(&case, &hierarchy, semantics, budget, true)?;
                }
            }
        }
    }
    Ok(())
}

fn measure(
    case: &Case,
    hierarchy: &SpatialHierarchy,
    semantics: HierarchySemantics,
    budget: u64,
    clipped: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let frustum = frustum(if clipped { 0.5 } else { 1.0 });
    // Zero tolerance intentionally asks for all available detail, unless budget-limited.
    let config = LodSelectionConfig::new(0.0, budget)?;
    for distance_model in [false, true] {
        let visited = RefCell::new(vec![false; case.nodes.len()]);
        let expected = hierarchy.select_lod(&frustum, config, |node| {
            visited.borrow_mut()[node.id().get() as usize] = true;
            project(node, distance_model)
        })?;
        let projected_nodes = visited.borrow().iter().filter(|visited| **visited).count();
        let mut checksum = 0xcbf29ce484222325_u64;
        for ids in [
            expected.node_ids(),
            expected.budget_limited(),
            expected.resolution_limited(),
        ] {
            for value in std::iter::once(ids.len() as u64).chain(ids.iter().map(|id| id.get())) {
                checksum = (checksum ^ value).wrapping_mul(0x100000001b3);
            }
        }
        for value in [
            expected.point_count(),
            expected.point_budget(),
            expected.max_error_pixels().to_bits(),
        ] {
            checksum = (checksum ^ value).wrapping_mul(0x100000001b3);
        }
        let mut times = Vec::with_capacity(SAMPLES);
        let mut callback_calls = None;
        for _ in 0..SAMPLES {
            let calls = Cell::new(0_u64);
            let start = Instant::now();
            let selection = hierarchy.select_lod(black_box(&frustum), config, |node| {
                calls.set(calls.get() + 1);
                project(black_box(node), distance_model)
            })?;
            times.push(start.elapsed().as_secs_f64() * 1_000_000.0);
            assert_eq!(selection, expected, "selection changed between samples");
            if let Some(previous) = callback_calls {
                assert_eq!(calls.get(), previous);
            }
            callback_calls = Some(calls.get());
            black_box(selection);
        }
        times.sort_unstable_by(f64::total_cmp);
        println!(
            "case={} semantics={semantics:?} view={} callback={} nodes={} budget={budget} selected_nodes={} selected_points={} budget_limited={} resolution_limited={} projected_nodes={projected_nodes} callback_calls={} selection_checksum={checksum} samples={SAMPLES} us_min={:.3} us_median={:.3} us_max={:.3}",
            case.name,
            if clipped { "half" } else { "all" },
            if distance_model {
                "distance_model"
            } else {
                "declared_error"
            },
            case.nodes.len(),
            expected.node_ids().len(),
            expected.point_count(),
            expected.budget_limited().len(),
            expected.resolution_limited().len(),
            callback_calls.unwrap(),
            times[0],
            times[SAMPLES / 2],
            times[SAMPLES - 1]
        );
    }
    Ok(())
}

fn project(node: &SpatialNode, distance_model: bool) -> f64 {
    if !distance_model {
        return node.geometric_error();
    }
    // Synthetic projection cost/priority, not the renderer's conservative camera projection.
    let bounds = node.bounds();
    let camera = [0.5, 0.5, 2.0];
    let distance_squared = (0..3)
        .map(|axis| {
            let delta = (bounds.min()[axis] + bounds.max()[axis]) * 0.5 - camera[axis];
            delta * delta
        })
        .sum::<f64>();
    node.geometric_error() * 640.0 / distance_squared.sqrt()
}

fn node(
    id: usize,
    parent: Option<usize>,
    level: u32,
    bounds: SpatialBounds,
    points: u64,
) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id as u64),
        parent.map(|id| NodeId::new(id as u64)),
        level,
        bounds,
        points,
        1.0 / f64::from(level + 1),
        if points == 0 {
            Vec::new()
        } else {
            vec![BlockId::new(id as u64)]
        },
    )
    .unwrap()
}

fn wide(branches: usize) -> Case {
    let root = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let mut nodes = vec![node(0, None, 0, root, 0)];
    for branch in 0..branches {
        let min = [branch as f64 / branches as f64, 0.0, 0.0];
        let max = [(branch + 1) as f64 / branches as f64, 1.0, 1.0];
        let id = nodes.len();
        nodes.push(node(
            id,
            Some(0),
            1,
            SpatialBounds::new(min, max).unwrap(),
            1,
        ));
        for child in 0..2 {
            let mut min = min;
            let mut max = max;
            min[1] = child as f64 * 0.5;
            max[1] = (child + 1) as f64 * 0.5;
            nodes.push(node(
                nodes.len(),
                Some(id),
                2,
                SpatialBounds::new(min, max).unwrap(),
                1,
            ));
        }
    }
    Case {
        name: format!("wide-{branches}"),
        nodes,
        base_points: branches as u64,
        leaf_points: (2 * branches) as u64,
        clip: true,
    }
}

fn binary(depth: u32) -> Case {
    let mut nodes: Vec<SpatialNode> = Vec::new();
    for id in 0..(1 << (depth + 1)) - 1 {
        let level = (id + 1_usize).ilog2();
        let parent = (id > 0).then(|| (id - 1) / 2);
        let bounds = if let Some(parent) = parent {
            let parent_bounds = nodes[parent].bounds();
            let axis = (level as usize - 1) % 3;
            let middle = (parent_bounds.min()[axis] + parent_bounds.max()[axis]) * 0.5;
            let mut min = parent_bounds.min();
            let mut max = parent_bounds.max();
            if id % 2 == 1 {
                max[axis] = middle;
            } else {
                min[axis] = middle;
            }
            SpatialBounds::new(min, max).unwrap()
        } else {
            SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap()
        };
        nodes.push(node(id, parent, level, bounds, 1));
    }
    Case {
        name: format!("binary-{depth}"),
        nodes,
        base_points: 1,
        leaf_points: 1 << depth,
        clip: true,
    }
}

fn chain(length: usize) -> Case {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let nodes = (0..length)
        .map(|id| node(id, id.checked_sub(1), id as u32, bounds, 1))
        .collect();
    Case {
        name: format!("chain-{length}"),
        nodes,
        base_points: 1,
        leaf_points: 1,
        clip: false,
    }
}

fn frustum(max_x: f64) -> SpatialFrustum {
    SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], max_x).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], 1.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], 1.0).unwrap(),
    ])
    .unwrap()
}
