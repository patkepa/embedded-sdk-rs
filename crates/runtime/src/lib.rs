#![no_std]
#![forbid(unsafe_code)]
#![doc = "Service lifecycle and health primitives shared by Embassy applications."]

/// Lifecycle state of a long-running device service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServiceState {
    /// The service has not started.
    Stopped,
    /// The service is initializing dependencies or hardware.
    Starting,
    /// The service is operating normally.
    Healthy,
    /// The service is operating with reduced functionality.
    Degraded,
    /// The service cannot perform its required function.
    Failed,
}

/// Snapshot used by supervisors and watchdog policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthSnapshot {
    /// Stable numeric service identifier.
    pub service_id: u16,
    /// Current lifecycle state.
    pub state: ServiceState,
    /// Monotonic tick of the most recent progress report.
    pub last_progress_tick: u64,
    /// Number of recoverable restarts since boot.
    pub restart_count: u16,
}

impl HealthSnapshot {
    /// Creates the initial snapshot for a service.
    #[must_use]
    pub const fn stopped(service_id: u16) -> Self {
        Self {
            service_id,
            state: ServiceState::Stopped,
            last_progress_tick: 0,
            restart_count: 0,
        }
    }

    /// Records progress and changes the current state.
    pub const fn report(&mut self, state: ServiceState, tick: u64) {
        self.state = state;
        self.last_progress_tick = tick;
    }

    /// Returns whether the service has exceeded its progress deadline.
    #[must_use]
    pub const fn is_stale(&self, now: u64, maximum_silence: u64) -> bool {
        now.saturating_sub(self.last_progress_tick) > maximum_silence
    }
}
