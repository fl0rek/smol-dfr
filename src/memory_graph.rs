use std::collections::VecDeque;
use std::time::Instant;

pub fn get_memory_usage() -> u32 {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut available = 0u64;
    for line in meminfo.lines() {
        if let Some(val) = line.strip_prefix("MemTotal:") {
            total = val.trim().strip_suffix(" kB").unwrap_or(val.trim())
                .trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("MemAvailable:") {
            available = val.trim().strip_suffix(" kB").unwrap_or(val.trim())
                .trim().parse().unwrap_or(0);
        }
    }
    if total == 0 {
        return 0;
    }
    ((total - available) * 100 / total) as u32
}

pub struct MemoryHistory {
    samples: VecDeque<u32>,
    max_samples: usize,
    sample_interval_ms: u32,
    last_sample: Instant,
}

impl MemoryHistory {
    pub fn new(sample_interval_ms: u32, graph_window_s: u32) -> Self {
        let max_samples = (graph_window_s as usize * 1000) / sample_interval_ms as usize;
        let mut history = Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
            sample_interval_ms,
            last_sample: Instant::now(),
        };
        // Take an initial sample immediately
        history.samples.push_back(get_memory_usage());
        history
    }

    /// Check if it's time to take a new sample. If so, read /proc/meminfo,
    /// push the sample, and return true.
    pub fn maybe_sample(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_sample).as_millis() < self.sample_interval_ms as u128 {
            return false;
        }
        self.last_sample = now;
        let usage = get_memory_usage();
        self.samples.push_back(usage);
        if self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
        true
    }

    pub fn samples(&self) -> &VecDeque<u32> {
        &self.samples
    }

    pub fn max_samples(&self) -> usize {
        self.max_samples
    }

    pub fn sample_interval_ms(&self) -> u32 {
        self.sample_interval_ms
    }
}
