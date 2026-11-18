//! Optional Linux process I/O accounting for the native conversion benchmark.
//! Intervals follow progress notifications, not internal converter function boundaries.

use std::{
    fs, io,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use viewer_convert::ConversionStage;

const IO_FIELDS: [&str; 5] = [
    "rchar",
    "wchar",
    "read_bytes",
    "write_bytes",
    "cancelled_write_bytes",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessIo([u64; IO_FIELDS.len()]);

impl ProcessIo {
    fn read() -> io::Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "convert-profile requires Linux /proc/self/io",
            ));
        }
        Self::parse(&fs::read_to_string("/proc/self/io")?)
    }

    fn parse(input: &str) -> io::Result<Self> {
        let mut values = [None; IO_FIELDS.len()];
        for line in input.lines() {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if let Some(index) = IO_FIELDS.iter().position(|field| *field == name) {
                if values[index].is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate I/O field",
                    ));
                }
                values[index] = Some(
                    value
                        .trim()
                        .parse::<u64>()
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
                );
            }
        }
        let mut result = [0; IO_FIELDS.len()];
        for (slot, value) in result.iter_mut().zip(values) {
            *slot = value.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing process I/O field")
            })?;
        }
        Ok(Self(result))
    }

    fn since(self, previous: Self) -> io::Result<Self> {
        let mut result = [0; IO_FIELDS.len()];
        for ((slot, current), previous) in result.iter_mut().zip(self.0).zip(previous.0) {
            *slot = current.checked_sub(previous).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "process I/O counter decreased")
            })?;
        }
        Ok(Self(result))
    }

    fn print(self, prefix: &str) {
        for (name, value) in IO_FIELDS.iter().zip(self.0) {
            println!("{prefix}_{name}={value}");
        }
    }
}

struct Interval {
    stage: Option<ConversionStage>,
    elapsed_ms: f64,
    io: Option<ProcessIo>,
    start_unix_ns: u128,
    end_unix_ns: u128,
}

pub(super) struct ConversionMetrics {
    current_stage: Option<ConversionStage>,
    started: Instant,
    started_unix_ns: u128,
    initial_io: Option<ProcessIo>,
    previous_io: Option<ProcessIo>,
    intervals: Vec<Interval>,
}

impl ConversionMetrics {
    pub(super) fn new(process_io: bool) -> io::Result<Self> {
        let initial_io = process_io.then(ProcessIo::read).transpose()?;
        Ok(Self {
            current_stage: None,
            started: Instant::now(),
            started_unix_ns: unix_ns()?,
            initial_io,
            previous_io: initial_io,
            intervals: Vec::new(),
        })
    }

    pub(super) fn observe(&mut self, stage: ConversionStage) -> io::Result<()> {
        if self.current_stage != Some(stage) {
            self.close_interval()?;
            self.current_stage = Some(stage);
        }
        Ok(())
    }

    fn close_interval(&mut self) -> io::Result<()> {
        let current_io = self.initial_io.map(|_| ProcessIo::read()).transpose()?;
        let now = Instant::now();
        let end_unix_ns = unix_ns()?;
        let delta = current_io
            .zip(self.previous_io)
            .map(|(current, previous)| current.since(previous))
            .transpose()?;
        self.intervals.push(Interval {
            stage: self.current_stage,
            elapsed_ms: now.duration_since(self.started).as_secs_f64() * 1000.0,
            io: delta,
            start_unix_ns: self.started_unix_ns,
            end_unix_ns,
        });
        self.previous_io = current_io;
        self.started = now;
        self.started_unix_ns = end_unix_ns;
        Ok(())
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        self.close_interval()
    }

    pub(super) fn print(&self) -> io::Result<()> {
        println!("process_io_enabled={}", self.initial_io.is_some());
        for (index, interval) in self.intervals.iter().enumerate() {
            let name = interval.stage.map_or_else(
                || "BeforeFirstProgress".to_owned(),
                |stage| format!("{stage:?}"),
            );
            // Retain legacy stage timing keys. Ordinals distinguish repeated stages.
            println!("stage_{name}_ms={:.3}", interval.elapsed_ms);
            if self.initial_io.is_some() {
                println!("phase_{index}_name={name}");
                println!("phase_{index}_ms={:.3}", interval.elapsed_ms);
                println!("phase_{index}_start_unix_ns={}", interval.start_unix_ns);
                println!("phase_{index}_end_unix_ns={}", interval.end_unix_ns);
                if let Some(counters) = interval.io {
                    counters.print(&format!("phase_{index}_io"));
                }
            }
        }
        if let Some((last, first)) = self.previous_io.zip(self.initial_io) {
            last.since(first)?.print("conversion_io");
        }
        Ok(())
    }
}

fn unix_ns() -> io::Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_parser_requires_complete_unsigned_counters_but_allows_other_fields() {
        let input = "wchar: 20\nrchar: 10\nsyscr: 3\nread_bytes: 0\nwrite_bytes: 4096\ncancelled_write_bytes: 1024\n";
        assert_eq!(
            ProcessIo::parse(input).unwrap(),
            ProcessIo([10, 20, 0, 4096, 1024])
        );
        for bad in [
            input.replace("read_bytes: 0\n", ""),
            input.replace("rchar: 10", "rchar: -1"),
            input.replace("rchar: 10", "rchar: 18446744073709551616"),
            format!("{input}rchar: 10\n"),
        ] {
            assert!(ProcessIo::parse(&bad).is_err());
        }
    }

    #[test]
    fn io_deltas_preserve_cancelled_writes_and_reject_counter_regression() {
        let first = ProcessIo([10, 20, 0, 4096, 0]);
        let second = ProcessIo([12, 40, 512, 8192, 4096]);
        assert_eq!(
            second.since(first).unwrap(),
            ProcessIo([2, 20, 512, 4096, 4096])
        );
        assert!(first.since(second).is_err());
    }

    #[test]
    fn timing_retains_repeated_stage_intervals_and_post_complete_cleanup() {
        let mut metrics = ConversionMetrics::new(false).unwrap();
        metrics
            .observe(ConversionStage::PlanningSpatialIndex)
            .unwrap();
        metrics
            .observe(ConversionStage::PlanningSpatialIndex)
            .unwrap();
        metrics
            .observe(ConversionStage::BuildingSpatialHierarchy)
            .unwrap();
        metrics
            .observe(ConversionStage::PlanningSpatialIndex)
            .unwrap();
        metrics.observe(ConversionStage::Complete).unwrap();
        metrics.finish().unwrap();
        let stages: Vec<_> = metrics
            .intervals
            .iter()
            .map(|interval| interval.stage)
            .collect();
        assert_eq!(
            stages,
            vec![
                None,
                Some(ConversionStage::PlanningSpatialIndex),
                Some(ConversionStage::BuildingSpatialHierarchy),
                Some(ConversionStage::PlanningSpatialIndex),
                Some(ConversionStage::Complete),
            ]
        );
        assert!(
            metrics
                .intervals
                .iter()
                .all(|interval| interval.io.is_none())
        );
        assert!(
            metrics
                .intervals
                .iter()
                .all(|interval| interval.end_unix_ns >= interval.start_unix_ns)
        );
        assert!(
            metrics
                .intervals
                .windows(2)
                .all(|pair| pair[0].end_unix_ns == pair[1].start_unix_ns)
        );
    }
}
