//! Shared I/O tuning for archive open/create/extract (runtime, not UI).

use std::time::{Duration, Instant};

/// Read/write chunk size. 128 KiB cuts syscall/IPC overhead vs 64 KiB on large files.
pub const IO_BUFFER_SIZE: usize = 128 * 1024;

/// Minimum gap between progress emissions (less frontend thrash on weak CPUs).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Rate-limits progress emissions to at most one per [`PROGRESS_INTERVAL`].
///
/// Construct with [`ProgressGate::new`]: the first [`should_emit`](Self::should_emit)
/// returns `true` immediately so callers always surface the first entry. Final 100%
/// progress should be emitted outside the gate (always).
#[derive(Debug)]
pub struct ProgressGate {
    last: Instant,
}

impl ProgressGate {
    /// Gate ready to emit immediately (first entry / first tick).
    pub fn new() -> Self {
        Self {
            last: Instant::now()
                .checked_sub(PROGRESS_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    /// Returns `true` when at least [`PROGRESS_INTERVAL`] has elapsed since the last emit.
    /// On `true`, updates the gate timestamp.
    pub fn should_emit(&mut self) -> bool {
        if self.last.elapsed() >= PROGRESS_INTERVAL {
            self.last = Instant::now();
            true
        } else {
            false
        }
    }
}

impl Default for ProgressGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn progress_gate_emits_first_then_throttles() {
        let mut gate = ProgressGate::new();
        assert!(gate.should_emit(), "first emit must pass");
        assert!(!gate.should_emit(), "immediate second emit must be gated");
        thread::sleep(PROGRESS_INTERVAL + Duration::from_millis(20));
        assert!(gate.should_emit(), "emit allowed after interval");
        assert!(!gate.should_emit(), "immediate follow-up still gated");
    }
}
