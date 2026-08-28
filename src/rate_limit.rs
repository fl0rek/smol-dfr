use std::time::{Duration, Instant};

/// A cached value that is refreshed at most once per `interval`.
pub struct RateLimitedValue<T> {
    value: T,
    last_read: Option<Instant>,
    interval: Duration,
}

impl<T> RateLimitedValue<T> {
    pub fn new(initial: T, interval: Duration) -> Self {
        Self {
            value: initial,
            last_read: None,
            interval,
        }
    }

    /// Call `read_fn` if the cached value is stale. Returns whether a refresh occurred.
    pub fn refresh_if_needed(&mut self, read_fn: impl FnOnce() -> T) -> bool {
        let stale = self
            .last_read
            .map_or(true, |t| t.elapsed() >= self.interval);
        if stale {
            self.value = read_fn();
            self.last_read = Some(Instant::now());
            true
        } else {
            false
        }
    }

    pub fn get(&self) -> &T {
        &self.value
    }
}

/// Logs a failure message once on transition to failed, and a recovery message
/// once on transition back to OK.
pub struct LogOnce {
    failed: bool,
    fail_msg: &'static str,
    recover_msg: &'static str,
}

impl LogOnce {
    pub fn new(fail_msg: &'static str, recover_msg: &'static str) -> Self {
        Self {
            failed: false,
            fail_msg,
            recover_msg,
        }
    }

    /// Update state and log on transitions.
    pub fn check(&mut self, ok: bool) {
        if ok && self.failed {
            eprintln!("{}", self.recover_msg);
            self.failed = false;
        } else if !ok && !self.failed {
            eprintln!("{}", self.fail_msg);
            self.failed = true;
        }
    }
}
