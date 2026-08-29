#![no_std]
#![forbid(unsafe_code)]
#![doc = "A backend-independent, allocation-free telemetry event model."]

/// Severity assigned to a telemetry event.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Severity {
    /// Detailed diagnostic information.
    Debug,
    /// Normal operational information.
    Info,
    /// A recoverable or degraded condition.
    Warning,
    /// A failed operation requiring attention.
    Error,
    /// A condition that prevents safe continued operation.
    Critical,
}

/// A compact event suitable for logs, metrics, or persistent crash records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    /// Stable numeric event code.
    pub code: u16,
    /// Event severity.
    pub severity: Severity,
    /// Component-specific signed value.
    pub value: i64,
}

/// Receives structured events without prescribing a logging transport.
pub trait EventSink {
    /// Sink-specific failure.
    type Error;

    /// Emits one event.
    fn emit(&mut self, event: Event) -> Result<(), Self::Error>;
}
