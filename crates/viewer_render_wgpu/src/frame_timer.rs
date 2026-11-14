//! GPU time of each rendered frame, measured with WebGPU timestamp queries.
//!
//! Two empty compute passes bracket the frame's work: the first writes timestamp 0 at its start,
//! the last writes timestamp 1 at its end. Pass-level writes need only `TIMESTAMP_QUERY`, the one
//! timestamp feature browsers expose. Results come back through a small ring of readback buffers
//! mapped asynchronously, so a frame never waits for them; when every slot is still in flight the
//! frame is simply not sampled (R3-Q31).

use std::collections::VecDeque;
use std::sync::mpsc;

/// Samples kept for the percentiles.
pub(crate) const WINDOW_CAPACITY: usize = 600;
/// Frames whose results can be in flight at once.
const RING_SLOTS: usize = 8;

/// Percentiles of the sampled GPU frame times, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuFrameStats {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub samples: usize,
    /// Frames not sampled since the last reset because every ring slot was still in flight. Slow
    /// frames stay in flight longer, so a high count means the percentiles may read low.
    pub dropped: u64,
    /// Greatest common divisor of the raw tick deltas, in ns: the timer's visible resolution. A
    /// value near 100 000 means the browser quantizes timestamps.
    pub resolution_ns: f64,
}

/// The last `capacity` samples; the oldest is dropped first.
#[derive(Debug)]
pub(crate) struct SampleWindow {
    samples: VecDeque<f64>,
    capacity: usize,
}

impl SampleWindow {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(crate) fn push(&mut self, value: f64) {
        if self.capacity == 0 {
            return;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    /// Nearest-rank percentiles; `None` when the window is empty.
    pub(crate) fn stats(&self) -> Option<GpuFrameStats> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = self.samples.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let rank = |fraction: f64| {
            let index = (fraction * sorted.len() as f64).ceil() as usize;
            sorted[index.clamp(1, sorted.len()) - 1]
        };
        Some(GpuFrameStats {
            p50_ms: rank(0.50),
            p95_ms: rank(0.95),
            max_ms: sorted[sorted.len() - 1],
            samples: sorted.len(),
            dropped: 0,
            resolution_ns: 0.0,
        })
    }
}

enum SlotState {
    Free,
    /// Between `begin` and `submitted`: the frame's commands are being recorded.
    Encoding,
    Pending(mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>),
}

struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: SlotState,
    /// Window generation the frame was started in; results of older ones are dropped.
    generation: u64,
}

/// Brackets frames with timestamps and collects the results without stalling.
pub(crate) struct FrameTimer {
    slots: Vec<Slot>,
    period_ns: f64,
    window: SampleWindow,
    enabled: bool,
    generation: u64,
    dropped: u64,
    tick_gcd: u64,
}

const TIMESTAMP_BYTES: u64 = 2 * size_of::<u64>() as u64;

impl FrameTimer {
    /// `None` when the device was created without `TIMESTAMP_QUERY`.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let slots = (0..RING_SLOTS)
            .map(|_| Slot {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("octree-viewer-frame-timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("octree-viewer-frame-timestamps-resolve"),
                    size: TIMESTAMP_BYTES,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("octree-viewer-frame-timestamps-readback"),
                    size: TIMESTAMP_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: SlotState::Free,
                generation: 0,
            })
            .collect();
        Some(Self {
            slots,
            period_ns: f64::from(queue.get_timestamp_period()),
            window: SampleWindow::new(WINDOW_CAPACITY),
            enabled: false,
            generation: 0,
            dropped: 0,
            tick_gcd: 0,
        })
    }

    /// Off by default: a disabled timer adds no passes, resolves or mappings to the frame.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Records the start timestamp. `None` when disabled, or (counted as dropped) when every slot
    /// is in flight.
    pub(crate) fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| matches!(slot.state, SlotState::Free))
        else {
            self.dropped += 1;
            return None;
        };
        let slot = &mut self.slots[index];
        slot.state = SlotState::Encoding;
        slot.generation = self.generation;
        drop(encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("octree-viewer-frame-start"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &slot.queries,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: None,
            }),
        }));
        Some(index)
    }

    /// Records the end timestamp and the copy to the readback buffer. Call after the frame's last
    /// pass, before `finish`.
    pub(crate) fn end(&self, encoder: &mut wgpu::CommandEncoder, index: usize) {
        let slot = &self.slots[index];
        drop(encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("octree-viewer-frame-end"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &slot.queries,
                beginning_of_pass_write_index: None,
                end_of_pass_write_index: Some(1),
            }),
        }));
        encoder.resolve_query_set(&slot.queries, 0..2, &slot.resolve, 0);
        encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, TIMESTAMP_BYTES);
    }

    /// Starts the asynchronous readback; call right after the queue submit.
    pub(crate) fn submitted(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        let (sender, receiver) = mpsc::channel();
        slot.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        slot.state = SlotState::Pending(receiver);
    }

    /// Moves finished readbacks into the window. Never blocks.
    pub(crate) fn collect(&mut self, device: &wgpu::Device) {
        if !self
            .slots
            .iter()
            .any(|slot| matches!(slot.state, SlotState::Pending(_)))
        {
            return;
        }
        // Native backends deliver map callbacks only while polled; the browser does it itself.
        #[cfg(not(target_arch = "wasm32"))]
        let _ = device.poll(wgpu::PollType::Poll);
        #[cfg(target_arch = "wasm32")]
        let _ = device;
        for slot in &mut self.slots {
            let SlotState::Pending(receiver) = &slot.state else {
                continue;
            };
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    if let Ok(view) = slot.readback.slice(..).get_mapped_range() {
                        let tick = |at: usize| {
                            u64::from_le_bytes(view[at..at + 8].try_into().expect("8 bytes"))
                        };
                        if slot.generation == self.generation
                            && let Some(delta) = tick(8).checked_sub(tick(0))
                        {
                            self.tick_gcd = gcd(self.tick_gcd, delta);
                            self.window.push(ticks_to_ms(delta, self.period_ns));
                        }
                    }
                    slot.readback.unmap();
                    slot.state = SlotState::Free;
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    slot.state = SlotState::Free;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub(crate) fn stats(&self) -> Option<GpuFrameStats> {
        self.window.stats().map(|stats| GpuFrameStats {
            dropped: self.dropped,
            resolution_ns: self.tick_gcd as f64 * self.period_ns,
            ..stats
        })
    }

    /// Starts a new window: frames still in flight belong to the old generation and are dropped.
    pub(crate) fn reset(&mut self) {
        self.window.clear();
        self.generation += 1;
        self.dropped = 0;
        self.tick_gcd = 0;
    }
}

fn ticks_to_ms(delta: u64, period_ns: f64) -> f64 {
    delta as f64 * period_ns / 1e6
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
#[path = "frame_timer_tests.rs"]
mod tests;
