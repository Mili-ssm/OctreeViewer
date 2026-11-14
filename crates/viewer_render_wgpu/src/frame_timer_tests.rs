use super::*;

fn filled(capacity: usize, values: impl IntoIterator<Item = f64>) -> SampleWindow {
    let mut window = SampleWindow::new(capacity);
    for value in values {
        window.push(value);
    }
    window
}

#[test]
fn empty_window_has_no_stats() {
    assert_eq!(SampleWindow::new(8).stats(), None);
}

#[test]
fn percentiles_use_nearest_rank() {
    let stats = filled(200, (1..=100).map(f64::from)).stats().unwrap();
    assert_eq!(stats.samples, 100);
    assert_eq!(stats.p50_ms, 50.0);
    assert_eq!(stats.p95_ms, 95.0);
    assert_eq!(stats.max_ms, 100.0);
}

#[test]
fn single_sample_is_every_percentile() {
    let stats = filled(4, [3.5]).stats().unwrap();
    assert_eq!((stats.p50_ms, stats.p95_ms, stats.max_ms), (3.5, 3.5, 3.5));
}

#[test]
fn window_keeps_only_the_newest_samples() {
    let stats = filled(3, [100.0, 1.0, 2.0, 3.0]).stats().unwrap();
    assert_eq!(stats.samples, 3);
    assert_eq!(stats.max_ms, 3.0);
}

#[test]
fn clear_empties_the_window() {
    let mut window = filled(3, [1.0, 2.0]);
    window.clear();
    assert_eq!(window.stats(), None);
}

#[test]
fn ticks_convert_with_the_period() {
    assert_eq!(ticks_to_ms(2_000, 500.0), 1.0);
}

#[test]
fn gcd_tracks_the_timer_resolution() {
    let gcd_of = |values: &[u64]| values.iter().fold(0, |acc, &v| gcd(acc, v));
    assert_eq!(gcd_of(&[]), 0);
    assert_eq!(gcd_of(&[300_000, 500_000, 1_100_000]), 100_000);
    assert_eq!(gcd_of(&[37, 41]), 1);
}

const HEAVY_SHADER: &str = "
@group(0) @binding(0) var<storage, read_write> sink: array<f32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    var x = f32(id.x) * 0.001;
    for (var i = 0u; i < 20000u; i++) { x = fma(x, 1.0001, sin(x) * 0.5); }
    sink[id.x % 64u] = x;
}";

struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
}

impl Rig {
    /// A frame bracketed by `timer`; `heavy` adds a large compute dispatch inside it.
    fn frame(&self, timer: &mut FrameTimer, heavy: bool, collect: bool) {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let slot = timer.begin(&mut encoder);
        if heavy {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(4096, 1, 1);
        }
        if let Some(slot) = slot {
            timer.end(&mut encoder, slot);
        }
        self.queue.submit([encoder.finish()]);
        if let Some(slot) = slot {
            timer.submitted(slot);
        }
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        if collect {
            timer.collect(&self.device);
        }
    }
}

/// `None` (with the reason printed) when no real adapter offers `timestamp-query`.
fn rig() -> Option<(Rig, FrameTimer)> {
    let instance = wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let Some(adapter) = adapters.into_iter().find(|adapter| {
        adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY)
            && adapter.get_info().device_type != wgpu::DeviceType::Cpu
    }) else {
        eprintln!("skipped: no real adapter offers timestamp-query");
        return None;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("frame-timer-test"),
        required_features: wgpu::Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))
    .expect("an adapter that lists timestamp-query must give a device with it");
    let timer = FrameTimer::new(&device, &queue).expect("timer");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(HEAVY_SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let sink = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: sink.as_entire_binding(),
        }],
    });
    Some((
        Rig {
            device,
            queue,
            pipeline,
            bind_group,
        },
        timer,
    ))
}

#[test]
fn a_disabled_timer_samples_nothing() {
    let Some((rig, mut timer)) = rig() else {
        return;
    };
    for _ in 0..4 {
        rig.frame(&mut timer, false, true);
    }
    assert_eq!(timer.stats(), None);
}

#[test]
fn heavy_frames_measure_longer_than_empty_ones() {
    let Some((rig, mut timer)) = rig() else {
        return;
    };
    timer.set_enabled(true);
    for _ in 0..16 {
        rig.frame(&mut timer, false, true);
    }
    let empty = timer
        .stats()
        .expect("samples on an adapter with the feature");
    assert!(empty.samples >= 8 && empty.max_ms < 1_000.0, "{empty:?}");
    assert!(empty.resolution_ns > 0.0, "{empty:?}");
    timer.reset();
    for _ in 0..16 {
        rig.frame(&mut timer, true, true);
    }
    let heavy = timer.stats().expect("samples");
    assert!(heavy.samples >= 8, "{heavy:?}");
    assert!(
        heavy.p50_ms > empty.p50_ms * 5.0 && heavy.p50_ms > 0.05,
        "empty {empty:?} heavy {heavy:?}"
    );
}

#[test]
fn frames_in_flight_at_a_reset_are_dropped_not_counted() {
    let Some((rig, mut timer)) = rig() else {
        return;
    };
    timer.set_enabled(true);
    for _ in 0..3 {
        rig.frame(&mut timer, false, false);
    }
    timer.reset();
    timer.collect(&rig.device);
    assert_eq!(timer.stats(), None, "old-generation samples leaked");
    rig.frame(&mut timer, false, true);
    assert_eq!(timer.stats().expect("new sample").samples, 1);
}

#[test]
fn a_full_ring_drops_frames_and_counts_them() {
    let Some((rig, mut timer)) = rig() else {
        return;
    };
    timer.set_enabled(true);
    for _ in 0..RING_SLOTS + 3 {
        rig.frame(&mut timer, false, false);
    }
    timer.collect(&rig.device);
    let stats = timer.stats().expect("the sampled frames");
    assert_eq!(stats.samples, RING_SLOTS);
    assert_eq!(stats.dropped, 3);
    timer.reset();
    rig.frame(&mut timer, false, true);
    assert_eq!(timer.stats().expect("sample").dropped, 0);
}
