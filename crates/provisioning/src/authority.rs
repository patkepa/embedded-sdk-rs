use crate::{DeviceState, SessionId};

/// Authenticated class of provisioning caller.
///
/// This value is metadata inside a trusted [`SessionContext`]. It must never
/// be accepted directly from a wire request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Authority {
    /// Automated hardware-in-the-loop fixture used during development.
    HilFixture,
    /// Authenticated owner performing device setup or recovery.
    OwnerSetup,
    /// Trusted manufacturing authority.
    Factory,
}

/// Broad operation class evaluated before a provisioning mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OperationKind {
    /// Read protocol capabilities or redacted device status.
    Inspect,
    /// Begin, submit, validate, or commit a configuration transaction.
    Configure,
    /// Abort the calling session's transient transaction.
    Abort,
    /// Remove provisioning state through the restartable reset procedure.
    FactoryReset,
}

/// Trusted in-process facts established by a transport or session adapter.
///
/// Construction is explicit so callers cannot accidentally treat decoded wire
/// fields as an authenticated context. The firmware composition root must only
/// call [`SessionContext::authenticated`] after its transport-specific checks
/// succeed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionContext {
    session_id: SessionId,
    authority: Authority,
    physical_presence: bool,
}

impl SessionContext {
    /// Creates a context after transport authentication has succeeded.
    #[must_use]
    pub const fn authenticated(
        session_id: SessionId,
        authority: Authority,
        physical_presence: bool,
    ) -> Self {
        Self {
            session_id,
            authority,
            physical_presence,
        }
    }

    /// Returns the connection-scoped session identifier.
    #[must_use]
    pub const fn session_id(self) -> SessionId {
        self.session_id
    }

    /// Returns the authenticated authority class.
    #[must_use]
    pub const fn authority(self) -> Authority {
        self.authority
    }

    /// Returns whether the adapter observed its configured presence signal.
    #[must_use]
    pub const fn has_physical_presence(self) -> bool {
        self.physical_presence
    }
}

/// Redacted reason an operation was not authorized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AuthorizationError {
    /// The authenticated authority cannot perform the operation.
    InsufficientAuthority,
    /// The operation requires a separately observed physical-presence signal.
    PhysicalPresenceRequired,
    /// Product policy forbids the operation in the current device state.
    InvalidDeviceState,
}

/// Product-owned authorization policy invoked by the provisioning service.
///
/// The policy receives only trusted session facts and redacted device state.
/// Product-specific configuration field authorization remains part of
/// candidate validation.
pub trait AuthorizationPolicy {
    /// Authorizes an operation before sensitive decoding or persistent writes.
    fn authorize(
        &self,
        session: &SessionContext,
        operation: OperationKind,
        device_state: DeviceState,
    ) -> Result<(), AuthorizationError>;
}
