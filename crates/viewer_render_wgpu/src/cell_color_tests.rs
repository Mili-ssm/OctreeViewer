//! Task 12 (R3-Q32): the shader colour modes of the cell shader against the CPU path's, on every
//! adapter. One cell and one voxel with the same inputs and ranges are drawn filling the view;
//! the centre pixel is compared. RGB and classification must match bit for bit, intensity and
//! elevation within 1 LSB per channel (R3-Q25). Elevation runs at an absolute height of about
//! 1000 with the eye at that height, the case the eye-relative offset exists for.

use super::*;
use crate::ColorMode;
use crate::cell_driver::CellColorRanges;
use crate::cell_render::CellColor;
use crate::test_support::mask_record;

const GRID: CellGrid = CellGrid {
    root_min: [0.0, 0.0, 990.0],
    root_side: 64.0,
};
/// Cell depth: sides of 1.
const DEPTH: u8 = 6;
const RGB: [u64; 3] = [200, 100, 50];

#[derive(Clone, Copy)]
struct Case {
    mode: ColorMode,
    /// Cell height index: the centre is at `990.5 + z`.
    z: u64,
    /// Mean intensity before the cell rounds it; `None` is a cell without intensity.
    intensity: Option<f64>,
    /// `-1` is missing.
    class: i16,
    ranges: CellColorRanges,
}

impl Case {
    fn new(mode: ColorMode) -> Self {
        Self {
            mode,
            z: 10,
            intensity: Some(1000.0),
            class: -1,
            ranges: CellColorRanges {
                intensity: Some([0.0, 4000.0]),
                z: Some([990.5, 1050.5]),
            },
        }
    }
}

fn cell_of(case: &Case) -> NodeCell {
    NodeCell {
        coord: [30, 30, case.z],
        rgb_sum: RGB,
        intensity_sum: case.intensity.unwrap_or(0.0),
        intensity_count: u32::from(case.intensity.is_some()),
        count: 1,
        classification: case.class,
        z_min: 0.0,
        z_max: 0.0,
    }
}

fn centre(image: &Image) -> [u8; 4] {
    image.color[(HEIGHT / 2 * WIDTH + WIDTH / 2) as usize]
}

fn run(h: &Harness, pool: &mut CellRenderer, case: &Case) -> ([u8; 4], [u8; 4]) {
    let cell = cell_of(case);
    let centre_world = [30.5, 30.5, 990.5 + case.z as f64];
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: centre_world.map(|v| v - 0.5),
            max: centre_world.map(|v| v + 0.5),
        },
        [WIDTH, HEIGHT]
    ));
    assert!(camera.orbit(0.9, 0.25));
    let bounds = SpatialBounds::new(GRID.root_min, GRID.root_min.map(|v| v + 64.0)).unwrap();
    let level = NodeCellLevel {
        depth: DEPTH,
        cells: vec![cell],
        octant_offsets: None,
    };
    let packed = pack_level(&GRID, bounds, NodeId::new(1), &level);
    let instance = packed.instances[0];
    // The CPU voxel takes what the cell packs: the same mean as `f32` before rounding.
    let intensity = case.intensity.map_or(-1.0, |mean| mean as f32);
    let voxel = InstanceRaw {
        center: centre_world.map(|v| v as f32),
        size: 1.0,
        color: ColorRgb::new(RGB[0] as u8, RGB[1] as u8, RGB[2] as u8).to_linear_rgba(),
        intensity,
        classification: f32::from(case.class),
    };
    assert_eq!(instance.class(), case.class);
    let (zmin, zmax) = case
        .ranges
        .z
        .map_or((0.0, 1.0), |z| (z[0] as f32, z[1] as f32));
    let range = case.ranges.intensity.unwrap_or([0.0, 1.0]);
    let cpu = render_cpu_styled(
        h,
        &[voxel],
        &camera,
        false,
        [case.mode.shader_value(), 1.0, zmin, zmax],
        range,
    );
    pool.free_pages(&[0]).unwrap();
    pool.upload_level(&packed, &[0]).unwrap();
    let mask = [0_u32; MASK_WORDS];
    pool.set_node_record(0, &mask_record(&mask)).unwrap();
    let frame = PageFrame {
        grid: GRID,
        coordinate_origin: [0.0; 3],
        visual_scale: 1.0,
        eye_scaled: camera.position(),
    };
    let entry = packed.pages[0].entry(&frame, 0, 0xff);
    pool.set_active_pages(&[(0, entry)]).unwrap();
    pool.set_color(CellColor::for_frame(
        case.mode,
        case.ranges,
        camera.position()[2],
        1.0,
        0.0,
    ));
    let (cells, drawn) = render_cells(h, pool, &camera, false);
    assert_eq!(drawn, 1, "the cell is drawn");
    (centre(&cells), centre(&cpu))
}

fn distance(a: [u8; 4], b: [u8; 4]) -> u8 {
    channel_distance(a, b)
}

#[test]
fn shader_modes_match_the_cpu_path_on_every_adapter() {
    let rgb = Case::new(ColorMode::Rgb);
    let mut cases: Vec<(&str, Case, u8)> = vec![("rgb", rgb, 0)];
    let intensity = Case::new(ColorMode::Intensity);
    let with = |case: Case, edit: &dyn Fn(&mut Case)| {
        let mut case = case;
        edit(&mut case);
        case
    };
    for (name, edit, limit) in [
        ("value", (&|_: &mut Case| {}) as &dyn Fn(&mut Case), 1),
        ("low end", &|c: &mut Case| c.intensity = Some(0.0), 1),
        ("high end", &|c: &mut Case| c.intensity = Some(4000.0), 1),
        ("above range", &|c: &mut Case| c.intensity = Some(5000.0), 1),
        (
            "below .5 LSB",
            &|c: &mut Case| c.intensity = Some(2000.49),
            1,
        ),
        (
            "above .5 LSB",
            &|c: &mut Case| c.intensity = Some(2000.51),
            1,
        ),
        (
            // One u16 unit is one 8-bit step of the ramp here, so a cell intensity off by 2 of
            // them (or truncated instead of rounded, 0.5 more on average) shows up.
            "narrow range, rounds down",
            &|c: &mut Case| {
                c.intensity = Some(100.49);
                c.ranges.intensity = Some([0.0, 255.0]);
            },
            1,
        ),
        (
            "narrow range, rounds up",
            &|c: &mut Case| {
                c.intensity = Some(100.51);
                c.ranges.intensity = Some([0.0, 255.0]);
            },
            1,
        ),
        ("missing", &|c: &mut Case| c.intensity = None, 1),
        (
            "span 0",
            &|c: &mut Case| {
                c.intensity = Some(500.0);
                c.ranges.intensity = Some([500.0, 500.0]);
            },
            1,
        ),
        ("no range", &|c: &mut Case| c.ranges.intensity = None, 1),
    ] {
        cases.push((name, with(intensity, edit), limit));
    }
    let elevation = Case::new(ColorMode::Elevation);
    for (name, z, range) in [
        ("low end", 0, [990.5, 1050.5]),
        ("high end", 60, [990.5, 1050.5]),
        ("middle", 30, [990.5, 1050.5]),
        ("below range", 0, [991.5, 1050.5]),
        ("above range", 63, [990.5, 1050.5]),
        ("span 0", 20, [1010.5, 1010.5]),
    ] {
        let mut case = elevation;
        case.z = z;
        case.ranges.z = Some(range);
        cases.push((name, case, 1));
    }
    for class in [-1, 0, 1, 2, 6, 9, 17, 255] {
        let mut case = Case::new(ColorMode::Classification);
        case.class = class;
        cases.push(("class", case, 0));
    }
    for mode in [ColorMode::Rgb, ColorMode::Intensity] {
        let mut case = Case::new(mode);
        case.class = 5;
        cases.push(("with class", case, u8::from(mode != ColorMode::Rgb)));
    }
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            println!(
                "# adapter {}: skipped (its limits do not admit the test pool)",
                h.label
            );
            return false;
        }
        let mut pool = new_pool(h);
        assert_eq!(pool.alloc_mask_slot(), Some(0));
        let mut worst = std::collections::BTreeMap::<&str, u8>::new();
        for (name, case, limit) in &cases {
            let (cells, cpu) = run(h, &mut pool, case);
            let diff = distance(cells, cpu);
            let key = case.mode.as_str();
            worst
                .entry(key)
                .and_modify(|d| *d = (*d).max(diff))
                .or_insert(diff);
            assert!(
                diff <= *limit,
                "{} {name}: cells {cells:?}, CPU {cpu:?}",
                case.mode.as_str()
            );
        }
        println!(
            "# adapter {}: max channel difference per mode {worst:?}",
            h.label
        );
        true
    });
}
