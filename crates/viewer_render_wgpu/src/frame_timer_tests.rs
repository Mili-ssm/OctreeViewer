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
fn ticks_convert_with_the_period_and_reject_reversed_pairs() {
    assert_eq!(ticks_to_ms(1_000, 3_000, 500.0), Some(1.0));
    assert_eq!(ticks_to_ms(3_000, 1_000, 500.0), None);
}

/// Real GPU: frames bracketed on a device with `TIMESTAMP_QUERY` yield plausible samples. Skips
/// where no adapter offers the feature (software rasterizers, most CI).
#[test]
fn bracketed_frames_produce_plausible_samples() {
    let instance = wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let Some(adapter) = adapters.into_iter().find(|adapter| {
        adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY)
            && adapter.get_info().device_type != wgpu::DeviceType::Cpu
    }) else {
        eprintln!("skipped: no adapter with timestamp-query");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("frame-timer-test"),
        required_features: wgpu::Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))
    .expect("device with timestamp-query");
    let mut timer = FrameTimer::new(&device, &queue).expect("timer");
    for _ in 0..20 {
        let mut encoder = device.create_command_encoder(&Default::default());
        let slot = timer.begin(&mut encoder);
        if let Some(slot) = slot {
            timer.end(&mut encoder, slot);
        }
        queue.submit([encoder.finish()]);
        if let Some(slot) = slot {
            timer.submitted(slot);
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        timer.collect(&device);
    }
    let stats = timer.stats().expect("samples");
    assert!(stats.samples >= 10, "{stats:?}");
    assert!(stats.p50_ms >= 0.0 && stats.max_ms < 1_000.0, "{stats:?}");
    timer.reset();
    assert_eq!(timer.stats(), None);
}
