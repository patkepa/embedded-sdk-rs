use core::fmt;

use embedded_sdk_cloud_core::{
    Capabilities as CloudCapabilities, ConnectionState, ErrorKind, Snapshot,
};
use embedded_sdk_mqtt::{
    ErrorKind as MqttErrorKind, MqttSession, OperationId, QoS, SessionCapabilities, SessionEvent,
    TopicFilter, TopicName,
};

use crate::{
    CloudToDeviceMessage, CodecError, DesiredPropertiesPatch, DirectMethodRequest, HubConfig,
    MethodRequestId, RequestId, RequestIdGenerator, TwinResponse, desired_properties_filter,
    direct_method_filter, direct_method_response_topic, parse_cloud_to_device,
    parse_desired_properties_patch, parse_direct_method, parse_twin_response,
    reported_properties_topic, twin_get_topic, twin_response_filter,
};

/// Azure IoT Hub operations enabled for one device session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HubCapabilities(u8);

impl HubCapabilities {
    /// Device-to-cloud telemetry.
    pub const TELEMETRY: Self = Self(1 << 0);
    /// Cloud-to-device messages.
    pub const CLOUD_TO_DEVICE: Self = Self(1 << 1);
    /// Direct-method requests and responses.
    pub const DIRECT_METHODS: Self = Self(1 << 2);
    /// Complete twin reads, desired patches, and reported properties.
    pub const TWINS: Self = Self(1 << 3);

    /// Creates an empty capability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns the union of two capability sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether every requested capability is enabled.
    #[must_use]
    pub const fn contains(self, requested: Self) -> bool {
        self.0 & requested.0 == requested.0
    }

    /// Maps provider operations to the deliberately smaller cloud capability set.
    #[must_use]
    pub const fn portable(self) -> CloudCapabilities {
        let mut capabilities = CloudCapabilities::empty();
        if self.contains(Self::TELEMETRY) {
            capabilities = capabilities.union(CloudCapabilities::TELEMETRY);
        }
        if self.contains(Self::CLOUD_TO_DEVICE) {
            capabilities = capabilities.union(CloudCapabilities::CLOUD_TO_DEVICE);
        }
        if self.contains(Self::DIRECT_METHODS) {
            capabilities = capabilities.union(CloudCapabilities::COMMANDS);
        }
        if self.contains(Self::TWINS) {
            capabilities = capabilities.union(CloudCapabilities::STATE_SYNCHRONIZATION);
        }
        capabilities
    }
}

/// MQTT broker session state reported after Azure CONNECT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionDisposition {
    /// The broker did not retain subscriptions for this client identity.
    Fresh,
    /// The broker retained the persistent MQTT session.
    Resumed,
}

/// One Azure subscription installed during fresh-session synchronization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Subscription {
    kind: SubscriptionKind,
    filter: TopicFilter,
}

impl Subscription {
    /// Returns the Azure operation enabled by this filter.
    #[must_use]
    pub const fn kind(self) -> SubscriptionKind {
        self.kind
    }

    /// Returns the MQTT topic filter.
    #[must_use]
    pub const fn filter(&self) -> &TopicFilter {
        &self.filter
    }

    /// Azure inbound operations use QoS 1 so acceptance can be explicit.
    #[must_use]
    pub const fn qos(self) -> QoS {
        QoS::AtLeastOnce
    }
}

/// Purpose of an Azure MQTT subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionKind {
    /// Device-specific cloud-to-device messages.
    CloudToDevice,
    /// Direct-method requests.
    DirectMethods,
    /// Responses for twin request-response operations.
    TwinResponses,
    /// Online desired-property patches.
    DesiredProperties,
}

/// Typed Azure operation decoded from an inbound MQTT publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HubEvent<'a> {
    /// A cloud-to-device message requiring application acceptance.
    CloudToDevice(CloudToDeviceMessage<'a>),
    /// A direct-method invocation requiring an application response.
    DirectMethod(DirectMethodRequest<'a>),
    /// A desired-property patch delivered while connected.
    DesiredPropertiesPatch(DesiredPropertiesPatch<'a>),
    /// A correlated response to a twin request.
    TwinResponse {
        /// Request type correlated by the provider.
        operation: TwinOperation,
        /// Parsed service response.
        response: TwinResponse<'a>,
    },
}

/// Correlated Azure device-twin request type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TwinOperation {
    /// Complete desired and reported document read.
    Get,
    /// Reported-property patch submission.
    ReportedProperties,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InboundAcceptance {
    acknowledgement_required: bool,
    count_as_accepted: bool,
    desired_version: Option<u64>,
}

impl InboundAcceptance {
    const fn from_event(event: HubEvent<'_>, acknowledgement_required: bool) -> Self {
        match event {
            HubEvent::DesiredPropertiesPatch(patch) => Self {
                acknowledgement_required,
                count_as_accepted: true,
                desired_version: Some(patch.version()),
            },
            HubEvent::TwinResponse { .. } => Self {
                acknowledgement_required,
                count_as_accepted: false,
                desired_version: None,
            },
            HubEvent::CloudToDevice(_) | HubEvent::DirectMethod(_) => Self {
                acknowledgement_required,
                count_as_accepted: true,
                desired_version: None,
            },
        }
    }
}

/// Purpose of a provider-owned MQTT publication awaiting PUBACK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboundOperation {
    /// Device-to-cloud telemetry.
    Telemetry,
    /// Complete device-twin read after connection or reconnect.
    TwinGet,
    /// Reported-property patch.
    ReportedProperties,
    /// Direct-method response.
    DirectMethodResponse,
}

/// Event produced by [`HubSession`] while driving Azure over MQTT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HubSessionEvent<'a> {
    /// An Azure operation borrowing the MQTT receive buffer.
    ///
    /// The application must finish reading or copying the value and then call
    /// [`HubSession::accept_inbound`] before polling again.
    Inbound(HubEvent<'a>),
    /// A provider-owned outbound operation was acknowledged by IoT Hub.
    OutboundAcknowledged {
        /// MQTT packet identifier.
        operation: OperationId,
        /// Azure purpose of the publication.
        purpose: OutboundOperation,
    },
    /// One fresh-session subscription was accepted.
    SubscriptionAccepted {
        /// MQTT packet identifier.
        operation: OperationId,
        /// Azure operation enabled by the filter.
        kind: SubscriptionKind,
    },
    /// MQTT keepalive or another internal exchange made progress.
    Progress,
}

/// Failure while coordinating Azure provider state with a live MQTT session.
#[derive(Debug)]
#[non_exhaustive]
pub enum HubSessionError<E> {
    /// Azure configuration, state, or topic processing failed.
    Provider(HubError),
    /// The concrete MQTT backend failed.
    Mqtt {
        /// Stable MQTT failure category captured before moving the error.
        kind: MqttErrorKind,
        /// Backend-specific diagnostic.
        source: E,
    },
    /// Another outbound or subscription operation is still in flight.
    OperationInFlight,
    /// An inbound publication must be accepted before further polling.
    InboundAcceptancePending,
    /// No delivered inbound publication is awaiting acceptance.
    NoInboundToAccept,
    /// MQTT acknowledged an operation that the provider did not start.
    UnexpectedAcknowledgement,
    /// The broker granted a lower subscription QoS than required.
    SubscriptionRejected,
    /// The MQTT backend cannot preserve required Azure delivery semantics.
    UnsupportedBackend,
}

impl<E> HubSessionError<E> {
    /// Returns the provider-independent failure category.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Provider(error) => error.kind(),
            Self::Mqtt { kind, .. } => cloud_error_from_mqtt(*kind),
            Self::OperationInFlight | Self::InboundAcceptancePending | Self::NoInboundToAccept => {
                ErrorKind::Application
            }
            Self::UnexpectedAcknowledgement | Self::SubscriptionRejected => ErrorKind::Protocol,
            Self::UnsupportedBackend => ErrorKind::Configuration,
        }
    }
}

impl<E: fmt::Debug> fmt::Display for HubSessionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => write!(formatter, "Azure IoT session error: {error}"),
            Self::Mqtt { source, .. } => write!(formatter, "Azure IoT MQTT error: {source:?}"),
            Self::OperationInFlight => formatter.write_str("Azure IoT operation is in flight"),
            Self::InboundAcceptancePending => {
                formatter.write_str("Azure IoT inbound acceptance is pending")
            }
            Self::NoInboundToAccept => {
                formatter.write_str("no Azure IoT inbound operation awaits acceptance")
            }
            Self::UnexpectedAcknowledgement => {
                formatter.write_str("unexpected Azure IoT MQTT acknowledgement")
            }
            Self::SubscriptionRejected => {
                formatter.write_str("Azure IoT MQTT subscription was not granted at QoS 1")
            }
            Self::UnsupportedBackend => {
                formatter.write_str("MQTT backend lacks required Azure IoT capabilities")
            }
        }
    }
}

impl<E: fmt::Debug> core::error::Error for HubSessionError<E> {}

impl<E> From<HubError> for HubSessionError<E> {
    fn from(value: HubError) -> Self {
        Self::Provider(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingMqttOperation {
    Publish {
        operation: OperationId,
        purpose: OutboundOperation,
    },
    Subscribe {
        operation: OperationId,
        kind: SubscriptionKind,
    },
}

/// Azure IoT Hub coordinator over a concrete live MQTT session.
///
/// This type owns no packet or payload buffers. It maps portable MQTT events
/// into Azure operations, preserves explicit inbound acceptance, and keeps one
/// provider operation in flight to match the bounded MQTT 3.1.1 backend.
pub struct HubSession<'hub, S> {
    hub: &'hub mut HubClient,
    mqtt: S,
    pending: Option<PendingMqttOperation>,
    pending_inbound: Option<InboundAcceptance>,
    subscriptions_accepted: usize,
}

impl<'hub, S: MqttSession> HubSession<'hub, S> {
    /// Attaches a connected MQTT session and begins Azure synchronization.
    pub fn new(
        hub: &'hub mut HubClient,
        mqtt: S,
        disposition: SessionDisposition,
    ) -> Result<Self, HubSessionError<S::Error>> {
        let publishes = hub.capabilities().contains(HubCapabilities::TELEMETRY)
            || hub.capabilities().contains(HubCapabilities::DIRECT_METHODS)
            || hub.capabilities().contains(HubCapabilities::TWINS);
        let subscriptions = hub.subscription_count() > 0;
        let manual_inbound_ack = hub
            .capabilities()
            .contains(HubCapabilities::CLOUD_TO_DEVICE)
            || hub.capabilities().contains(HubCapabilities::TWINS);
        let mut required = SessionCapabilities::empty();
        if publishes {
            required = required.union(SessionCapabilities::CORRELATED_PUBLISH_ACK);
        }
        if subscriptions {
            required = required.union(SessionCapabilities::CORRELATED_SUBSCRIPTION_ACK);
        }
        if manual_inbound_ack {
            required = required.union(SessionCapabilities::MANUAL_INBOUND_ACK);
        }
        if !mqtt.capabilities().contains(required) {
            return Err(HubSessionError::UnsupportedBackend);
        }
        hub.connected(disposition);
        let subscriptions_accepted = if disposition == SessionDisposition::Resumed {
            hub.subscription_count()
        } else {
            0
        };
        Ok(Self {
            hub,
            mqtt,
            pending: None,
            pending_inbound: None,
            subscriptions_accepted,
        })
    }

    /// Returns the provider's current redacted health snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> Snapshot {
        self.hub.snapshot()
    }

    /// Starts the next required fresh-session subscription.
    ///
    /// `Ok(None)` means all required subscriptions are already accepted.
    pub async fn begin_next_subscription(
        &mut self,
        scratch: &mut [u8],
    ) -> Result<Option<OperationId>, HubSessionError<S::Error>> {
        self.ensure_idle()?;
        let Some(subscription) = self
            .hub
            .subscription(self.subscriptions_accepted, scratch)?
        else {
            return Ok(None);
        };
        let operation = match self
            .mqtt
            .subscribe(subscription.filter(), subscription.qos())
            .await
        {
            Ok(operation) => operation,
            Err(error) => return Err(self.observe_mqtt(error)),
        };
        self.pending = Some(PendingMqttOperation::Subscribe {
            operation,
            kind: subscription.kind(),
        });
        Ok(Some(operation))
    }

    /// Starts a QoS 1 telemetry publication.
    pub async fn publish_telemetry(
        &mut self,
        payload: &[u8],
        topic_scratch: &mut [u8],
    ) -> Result<OperationId, HubSessionError<S::Error>> {
        self.ensure_idle()?;
        if !self.hub.capabilities().contains(HubCapabilities::TELEMETRY) {
            return Err(HubSessionError::Provider(HubError::NotReady));
        }
        let topic = self
            .hub
            .config()
            .telemetry_topic(topic_scratch)
            .map_err(HubError::from)?;
        let operation = match self.mqtt.publish(&topic, payload, QoS::AtLeastOnce).await {
            Ok(Some(operation)) => operation,
            Ok(None) => return Err(HubSessionError::UnexpectedAcknowledgement),
            Err(error) => return Err(self.observe_mqtt(error)),
        };
        self.pending = Some(PendingMqttOperation::Publish {
            operation,
            purpose: OutboundOperation::Telemetry,
        });
        Ok(operation)
    }

    /// Starts the mandatory complete twin read after subscriptions are ready.
    pub async fn begin_twin_sync(
        &mut self,
        topic_scratch: &mut [u8],
    ) -> Result<OperationId, HubSessionError<S::Error>> {
        self.ensure_idle()?;
        let (_, topic) = self.hub.begin_twin_sync(topic_scratch)?;
        let operation = match self.mqtt.publish(&topic, &[], QoS::AtLeastOnce).await {
            Ok(Some(operation)) => operation,
            Ok(None) => return Err(HubSessionError::UnexpectedAcknowledgement),
            Err(error) => return Err(self.observe_mqtt(error)),
        };
        self.pending = Some(PendingMqttOperation::Publish {
            operation,
            purpose: OutboundOperation::TwinGet,
        });
        Ok(operation)
    }

    /// Starts a reported-property PATCH and correlates both MQTT and service acknowledgments.
    pub async fn publish_reported_properties(
        &mut self,
        payload: &[u8],
        topic_scratch: &mut [u8],
    ) -> Result<OperationId, HubSessionError<S::Error>> {
        self.ensure_idle()?;
        let (_, topic) = self.hub.begin_reported_properties(topic_scratch)?;
        let operation = match self.mqtt.publish(&topic, payload, QoS::AtLeastOnce).await {
            Ok(Some(operation)) => operation,
            Ok(None) => return Err(HubSessionError::UnexpectedAcknowledgement),
            Err(error) => return Err(self.observe_mqtt(error)),
        };
        self.pending = Some(PendingMqttOperation::Publish {
            operation,
            purpose: OutboundOperation::ReportedProperties,
        });
        Ok(operation)
    }

    /// Publishes a direct-method response using an owned request correlation ID.
    pub async fn respond_direct_method(
        &mut self,
        request_id: &MethodRequestId,
        status: u16,
        payload: &[u8],
        topic_scratch: &mut [u8],
    ) -> Result<OperationId, HubSessionError<S::Error>> {
        self.ensure_idle()?;
        if !self
            .hub
            .capabilities()
            .contains(HubCapabilities::DIRECT_METHODS)
            || !self.hub.subscriptions_ready
        {
            return Err(HubSessionError::Provider(HubError::NotReady));
        }
        let topic = direct_method_response_topic(request_id.as_str(), status, topic_scratch)
            .map_err(HubError::from)?;
        let operation = match self.mqtt.publish(&topic, payload, QoS::AtLeastOnce).await {
            Ok(Some(operation)) => operation,
            Ok(None) => return Err(HubSessionError::UnexpectedAcknowledgement),
            Err(error) => return Err(self.observe_mqtt(error)),
        };
        self.pending = Some(PendingMqttOperation::Publish {
            operation,
            purpose: OutboundOperation::DirectMethodResponse,
        });
        Ok(operation)
    }

    /// Drives one MQTT event and maps it into provider state.
    pub async fn poll(&mut self) -> Result<HubSessionEvent<'_>, HubSessionError<S::Error>> {
        if self.pending_inbound.is_some() {
            return Err(HubSessionError::InboundAcceptancePending);
        }
        let mqtt_event = match self.mqtt.poll().await {
            Ok(event) => event,
            Err(error) => {
                let kind = S::classify_error(&error);
                self.hub.failed(cloud_error_from_mqtt(kind));
                return Err(HubSessionError::Mqtt {
                    kind,
                    source: error,
                });
            }
        };
        match mqtt_event {
            SessionEvent::Publish(publication) => {
                let event = self
                    .hub
                    .handle_publish(publication.topic(), publication.payload())?;
                self.pending_inbound = Some(InboundAcceptance::from_event(
                    event,
                    publication.acknowledgement_required(),
                ));
                Ok(HubSessionEvent::Inbound(event))
            }
            SessionEvent::Published(operation) => {
                let Some(PendingMqttOperation::Publish {
                    operation: expected,
                    purpose,
                }) = self.pending
                else {
                    return Err(HubSessionError::UnexpectedAcknowledgement);
                };
                if operation != expected {
                    return Err(HubSessionError::UnexpectedAcknowledgement);
                }
                self.pending = None;
                self.hub.outbound_acknowledged();
                Ok(HubSessionEvent::OutboundAcknowledged { operation, purpose })
            }
            SessionEvent::Subscribed {
                operation,
                granted_qos,
            } => {
                let Some(PendingMqttOperation::Subscribe {
                    operation: expected,
                    kind,
                }) = self.pending
                else {
                    return Err(HubSessionError::UnexpectedAcknowledgement);
                };
                if operation != expected {
                    return Err(HubSessionError::UnexpectedAcknowledgement);
                }
                if granted_qos != QoS::AtLeastOnce {
                    return Err(HubSessionError::SubscriptionRejected);
                }
                self.pending = None;
                self.subscriptions_accepted = self.subscriptions_accepted.saturating_add(1);
                if self.subscriptions_accepted == self.hub.subscription_count() {
                    self.hub.subscriptions_ready()?;
                }
                Ok(HubSessionEvent::SubscriptionAccepted { operation, kind })
            }
            SessionEvent::Progress => Ok(HubSessionEvent::Progress),
        }
    }

    /// Commits the last inbound operation at the application ownership boundary.
    ///
    /// For QoS 1 this records acceptance before transmitting PUBACK. If the ACK
    /// fails, the application may observe a redelivery after reconnect and must
    /// apply its own idempotency policy.
    pub async fn accept_inbound(&mut self) -> Result<(), HubSessionError<S::Error>> {
        let pending = self
            .pending_inbound
            .take()
            .ok_or(HubSessionError::NoInboundToAccept)?;
        self.hub.record_inbound_acceptance(pending);
        if pending.acknowledgement_required
            && let Err(error) = self.mqtt.acknowledge_received().await
        {
            return Err(self.observe_mqtt(error));
        }
        Ok(())
    }

    /// Gracefully disconnects the composed MQTT session.
    pub async fn disconnect(&mut self) -> Result<(), HubSessionError<S::Error>> {
        match self.mqtt.disconnect().await {
            Ok(()) => {
                self.hub.transition(ConnectionState::WaitingForNetwork);
                Ok(())
            }
            Err(error) => Err(self.observe_mqtt(error)),
        }
    }

    /// Releases the provider and concrete MQTT session.
    #[must_use]
    pub fn into_parts(self) -> (&'hub mut HubClient, S) {
        (self.hub, self.mqtt)
    }

    fn ensure_idle(&self) -> Result<(), HubSessionError<S::Error>> {
        if self.pending.is_some() {
            Err(HubSessionError::OperationInFlight)
        } else if self.pending_inbound.is_some() {
            Err(HubSessionError::InboundAcceptancePending)
        } else {
            Ok(())
        }
    }

    fn observe_mqtt(&mut self, error: S::Error) -> HubSessionError<S::Error> {
        let kind = S::classify_error(&error);
        self.hub.failed(cloud_error_from_mqtt(kind));
        HubSessionError::Mqtt {
            kind,
            source: error,
        }
    }
}

const fn cloud_error_from_mqtt(error: MqttErrorKind) -> ErrorKind {
    match error {
        MqttErrorKind::Configuration => ErrorKind::Configuration,
        MqttErrorKind::Capacity => ErrorKind::Capacity,
        MqttErrorKind::Transport => ErrorKind::Network,
        MqttErrorKind::Authentication => ErrorKind::Authentication,
        MqttErrorKind::Protocol | MqttErrorKind::Disconnected | MqttErrorKind::NotReady => {
            ErrorKind::Protocol
        }
        _ => ErrorKind::Protocol,
    }
}

/// Azure provider orchestration failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HubError {
    /// An Azure topic or property failed strict decoding.
    Codec(CodecError),
    /// An operation was attempted before connection synchronization allowed it.
    NotReady,
    /// Only one twin request may be correlated by this first bounded slice.
    OperationInFlight,
    /// A twin response did not match the outstanding request.
    UnexpectedResponse,
    /// IoT Hub returned a non-success status for a correlated request.
    ServiceRejected(u16),
}

impl HubError {
    /// Returns the provider-independent failure category.
    #[must_use]
    pub const fn kind(self) -> ErrorKind {
        match self {
            Self::Codec(_) | Self::UnexpectedResponse => ErrorKind::Protocol,
            Self::NotReady | Self::OperationInFlight => ErrorKind::Application,
            Self::ServiceRejected(429) => ErrorKind::Throttled,
            Self::ServiceRejected(_) => ErrorKind::ServiceRejected,
        }
    }
}

impl fmt::Display for HubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => write!(formatter, "Azure IoT codec failure: {error}"),
            Self::NotReady => formatter.write_str("Azure IoT session is not ready"),
            Self::OperationInFlight => formatter.write_str("Azure IoT operation is in flight"),
            Self::UnexpectedResponse => {
                formatter.write_str("unexpected Azure IoT service response")
            }
            Self::ServiceRejected(status) => {
                write!(
                    formatter,
                    "Azure IoT service rejected operation with {status}"
                )
            }
        }
    }
}

impl core::error::Error for HubError {}

impl From<CodecError> for HubError {
    fn from(value: CodecError) -> Self {
        Self::Codec(value)
    }
}

/// Allocation-free Azure IoT Hub provider state independent of a concrete MQTT backend.
pub struct HubClient {
    config: HubConfig,
    capabilities: HubCapabilities,
    snapshot: Snapshot,
    subscriptions_ready: bool,
    request_ids: RequestIdGenerator,
    pending_twin_request: Option<(RequestId, TwinOperation)>,
    desired_version: Option<u64>,
}

impl HubClient {
    /// Creates a provider session in the waiting-for-network state.
    #[must_use]
    pub fn new(config: HubConfig, capabilities: HubCapabilities) -> Self {
        let mut snapshot = Snapshot::default();
        snapshot.transition(ConnectionState::WaitingForNetwork);
        Self {
            config,
            capabilities,
            snapshot,
            subscriptions_ready: false,
            request_ids: RequestIdGenerator::new(1),
            pending_twin_request: None,
            desired_version: None,
        }
    }

    /// Returns immutable connection configuration.
    #[must_use]
    pub const fn config(&self) -> &HubConfig {
        &self.config
    }

    /// Returns enabled Azure operations.
    #[must_use]
    pub const fn capabilities(&self) -> HubCapabilities {
        self.capabilities
    }

    /// Returns redacted portable health and activity state.
    #[must_use]
    pub const fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    /// Returns the last accepted desired-property version.
    #[must_use]
    pub const fn desired_version(&self) -> Option<u64> {
        self.desired_version
    }

    /// Records a caller-owned lifecycle transition before MQTT CONNECT.
    pub const fn transition(&mut self, state: ConnectionState) {
        self.snapshot.transition(state);
    }

    /// Records MQTT connection establishment and determines resynchronization work.
    pub const fn connected(&mut self, disposition: SessionDisposition) {
        self.snapshot.record_connected();
        self.subscriptions_ready =
            matches!(disposition, SessionDisposition::Resumed) || self.subscription_count() == 0;
        self.pending_twin_request = None;
        if !self.subscriptions_ready || self.capabilities.contains(HubCapabilities::TWINS) {
            self.snapshot.transition(ConnectionState::Synchronizing);
        }
    }

    /// Returns how many Azure filters a fresh broker session must install.
    #[must_use]
    pub const fn subscription_count(&self) -> usize {
        let mut count = 0;
        if self.capabilities.contains(HubCapabilities::CLOUD_TO_DEVICE) {
            count += 1;
        }
        if self.capabilities.contains(HubCapabilities::DIRECT_METHODS) {
            count += 1;
        }
        if self.capabilities.contains(HubCapabilities::TWINS) {
            count += 2;
        }
        count
    }

    /// Builds the indexed subscription in deterministic synchronization order.
    pub fn subscription(
        &self,
        mut index: usize,
        scratch: &mut [u8],
    ) -> Result<Option<Subscription>, HubError> {
        if self.capabilities.contains(HubCapabilities::CLOUD_TO_DEVICE) {
            if index == 0 {
                return Ok(Some(Subscription {
                    kind: SubscriptionKind::CloudToDevice,
                    filter: self.config.cloud_to_device_filter(scratch)?,
                }));
            }
            index -= 1;
        }
        if self.capabilities.contains(HubCapabilities::DIRECT_METHODS) {
            if index == 0 {
                return Ok(Some(Subscription {
                    kind: SubscriptionKind::DirectMethods,
                    filter: direct_method_filter()?,
                }));
            }
            index -= 1;
        }
        if self.capabilities.contains(HubCapabilities::TWINS) {
            let (kind, filter) = match index {
                0 => (SubscriptionKind::TwinResponses, twin_response_filter()?),
                1 => (
                    SubscriptionKind::DesiredProperties,
                    desired_properties_filter()?,
                ),
                _ => return Ok(None),
            };
            return Ok(Some(Subscription { kind, filter }));
        }
        Ok(None)
    }

    /// Marks all fresh-session subscriptions accepted by the broker.
    pub const fn subscriptions_ready(&mut self) -> Result<(), HubError> {
        if !matches!(self.snapshot.state, ConnectionState::Synchronizing) {
            return Err(HubError::NotReady);
        }
        self.subscriptions_ready = true;
        if !self.capabilities.contains(HubCapabilities::TWINS) {
            self.snapshot.transition(ConnectionState::Online);
        }
        Ok(())
    }

    /// Starts the mandatory complete twin read performed after every reconnect.
    pub fn begin_twin_sync(
        &mut self,
        output: &mut [u8],
    ) -> Result<(RequestId, TopicName), HubError> {
        if !self.capabilities.contains(HubCapabilities::TWINS) || !self.subscriptions_ready {
            return Err(HubError::NotReady);
        }
        if self.pending_twin_request.is_some() {
            return Err(HubError::OperationInFlight);
        }
        let request_id = self.request_ids.allocate();
        let topic = twin_get_topic(request_id, output)?;
        self.pending_twin_request = Some((request_id, TwinOperation::Get));
        self.snapshot.transition(ConnectionState::Synchronizing);
        Ok((request_id, topic))
    }

    /// Starts a correlated reported-property update.
    pub fn begin_reported_properties(
        &mut self,
        output: &mut [u8],
    ) -> Result<(RequestId, TopicName), HubError> {
        if !self.capabilities.contains(HubCapabilities::TWINS)
            || !self.subscriptions_ready
            || !matches!(self.snapshot.state, ConnectionState::Online)
        {
            return Err(HubError::NotReady);
        }
        if self.pending_twin_request.is_some() {
            return Err(HubError::OperationInFlight);
        }
        let request_id = self.request_ids.allocate();
        let topic = reported_properties_topic(request_id, output)?;
        self.pending_twin_request = Some((request_id, TwinOperation::ReportedProperties));
        Ok((request_id, topic))
    }

    /// Decodes an MQTT publication, enforces enabled features, and correlates twin sync.
    pub fn handle_publish<'a>(
        &mut self,
        topic: &'a str,
        payload: &'a [u8],
    ) -> Result<HubEvent<'a>, HubError> {
        if topic.starts_with("devices/")
            && self.capabilities.contains(HubCapabilities::CLOUD_TO_DEVICE)
        {
            return Ok(HubEvent::CloudToDevice(parse_cloud_to_device(
                &self.config,
                topic,
                payload,
            )?));
        }
        if topic.starts_with("$iothub/methods/POST/")
            && self.capabilities.contains(HubCapabilities::DIRECT_METHODS)
        {
            return Ok(HubEvent::DirectMethod(parse_direct_method(topic, payload)?));
        }
        if topic.starts_with("$iothub/twin/PATCH/properties/desired/")
            && self.capabilities.contains(HubCapabilities::TWINS)
        {
            return Ok(HubEvent::DesiredPropertiesPatch(
                parse_desired_properties_patch(topic, payload)?,
            ));
        }
        if topic.starts_with("$iothub/twin/res/")
            && self.capabilities.contains(HubCapabilities::TWINS)
        {
            let response = parse_twin_response(topic, payload)?;
            let (request, operation) = self
                .pending_twin_request
                .ok_or(HubError::UnexpectedResponse)?;
            if !request.matches(response.request_id()) {
                return Err(HubError::UnexpectedResponse);
            }
            self.pending_twin_request = None;
            if !(200..300).contains(&response.status()) {
                let error = HubError::ServiceRejected(response.status());
                self.snapshot.record_failure(error.kind());
                return Err(error);
            }
            if operation == TwinOperation::Get {
                self.snapshot.transition(ConnectionState::Online);
            }
            return Ok(HubEvent::TwinResponse {
                operation,
                response,
            });
        }
        Err(HubError::Codec(CodecError::UnexpectedTopic))
    }

    /// Records acceptance at the owning application queue boundary.
    pub const fn accept_inbound(&mut self, event: HubEvent<'_>) {
        self.record_inbound_acceptance(InboundAcceptance::from_event(event, false));
    }

    /// Records a broker-acknowledged outbound MQTT publication.
    pub const fn outbound_acknowledged(&mut self) {
        self.snapshot.record_outbound_acknowledged();
    }

    /// Records a bounded queue rejection without exposing message content.
    pub const fn queue_dropped(&mut self) {
        self.snapshot.record_queue_drop();
    }

    /// Records a normalized recoverable failure and clears transient correlation.
    pub const fn failed(&mut self, error: ErrorKind) {
        self.pending_twin_request = None;
        self.subscriptions_ready = false;
        self.snapshot.record_failure(error);
    }

    const fn record_inbound_acceptance(&mut self, acceptance: InboundAcceptance) {
        if acceptance.count_as_accepted {
            self.snapshot.record_inbound_accepted();
        }
        if let Some(version) = acceptance.desired_version
            && match self.desired_version {
                Some(current) => version > current,
                None => true,
            }
        {
            self.desired_version = Some(version);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{future::Future, task::Poll};

    use std::{
        collections::VecDeque,
        task::{Context, Waker},
    };

    use super::*;
    use crate::{DeviceId, HubHostname};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct MockError(MqttErrorKind);

    struct MockSession {
        events: VecDeque<SessionEvent<'static>>,
        next_id: u16,
        subscriptions: usize,
        publishes: usize,
        acknowledgements: usize,
        disconnected: bool,
        twin_response_after_publish: bool,
        capabilities: SessionCapabilities,
    }

    impl MockSession {
        fn new(events: impl IntoIterator<Item = SessionEvent<'static>>) -> Self {
            Self {
                events: events.into_iter().collect(),
                next_id: 1,
                subscriptions: 0,
                publishes: 0,
                acknowledgements: 0,
                disconnected: false,
                twin_response_after_publish: false,
                capabilities: SessionCapabilities::MANUAL_INBOUND_ACK
                    .union(SessionCapabilities::CORRELATED_PUBLISH_ACK)
                    .union(SessionCapabilities::CORRELATED_SUBSCRIPTION_ACK),
            }
        }

        fn with_twin_response() -> Self {
            Self {
                twin_response_after_publish: true,
                ..Self::new([])
            }
        }

        fn allocate(&mut self) -> OperationId {
            let operation = OperationId::new(self.next_id).unwrap();
            self.next_id += 1;
            operation
        }
    }

    impl MqttSession for MockSession {
        type Error = MockError;

        fn capabilities(&self) -> SessionCapabilities {
            self.capabilities
        }

        fn classify_error(error: &Self::Error) -> MqttErrorKind {
            error.0
        }

        async fn subscribe(
            &mut self,
            _filter: &TopicFilter,
            _qos: QoS,
        ) -> Result<OperationId, Self::Error> {
            let operation = self.allocate();
            self.subscriptions += 1;
            self.events.push_back(SessionEvent::Subscribed {
                operation,
                granted_qos: QoS::AtLeastOnce,
            });
            Ok(operation)
        }

        async fn publish(
            &mut self,
            _topic: &TopicName,
            _payload: &[u8],
            _qos: QoS,
        ) -> Result<Option<OperationId>, Self::Error> {
            let operation = self.allocate();
            self.publishes += 1;
            self.events.push_back(SessionEvent::Published(operation));
            if self.twin_response_after_publish {
                self.twin_response_after_publish = false;
                self.events.push_back(SessionEvent::Publish(
                    embedded_sdk_mqtt::InboundPublish::new(
                        "$iothub/twin/res/200/?$rid=1&$version=7",
                        b"{}",
                        QoS::AtMostOnce,
                        false,
                        false,
                    ),
                ));
            }
            Ok(Some(operation))
        }

        async fn poll(&mut self) -> Result<SessionEvent<'_>, Self::Error> {
            self.events
                .pop_front()
                .ok_or(MockError(MqttErrorKind::NotReady))
        }

        async fn acknowledge_received(&mut self) -> Result<(), Self::Error> {
            self.acknowledgements += 1;
            Ok(())
        }

        async fn disconnect(&mut self) -> Result<(), Self::Error> {
            self.disconnected = true;
            Ok(())
        }
    }

    fn config() -> HubConfig {
        HubConfig::new(
            HubHostname::new("contoso.azure-devices.net").unwrap(),
            DeviceId::new("sensor-01").unwrap(),
            240,
            1024,
        )
        .unwrap()
    }

    fn all_capabilities() -> HubCapabilities {
        HubCapabilities::TELEMETRY
            .union(HubCapabilities::CLOUD_TO_DEVICE)
            .union(HubCapabilities::DIRECT_METHODS)
            .union(HubCapabilities::TWINS)
    }

    #[test]
    fn fresh_session_lists_four_bounded_subscriptions_before_twin_sync() {
        let mut client = HubClient::new(config(), all_capabilities());
        client.connected(SessionDisposition::Fresh);
        assert_eq!(client.snapshot().state, ConnectionState::Synchronizing);
        assert_eq!(client.subscription_count(), 4);

        let mut scratch = [0; 256];
        let kinds = [
            SubscriptionKind::CloudToDevice,
            SubscriptionKind::DirectMethods,
            SubscriptionKind::TwinResponses,
            SubscriptionKind::DesiredProperties,
        ];
        for (index, expected) in kinds.into_iter().enumerate() {
            assert_eq!(
                client
                    .subscription(index, &mut scratch)
                    .unwrap()
                    .unwrap()
                    .kind(),
                expected
            );
        }
        assert!(client.subscription(4, &mut scratch).unwrap().is_none());
        assert_eq!(
            client.begin_twin_sync(&mut scratch),
            Err(HubError::NotReady)
        );
        client.subscriptions_ready().unwrap();
        assert!(client.begin_twin_sync(&mut scratch).is_ok());
    }

    #[test]
    fn reconnect_requires_and_correlates_complete_twin_read() {
        let mut client = HubClient::new(config(), HubCapabilities::TWINS);
        client.connected(SessionDisposition::Resumed);
        let mut output = [0; 64];
        let (request, topic) = client.begin_twin_sync(&mut output).unwrap();
        assert_eq!(topic.as_str(), "$iothub/twin/GET/?$rid=1");
        assert_eq!(
            client.handle_publish("$iothub/twin/res/200/?$rid=2", b"{}"),
            Err(HubError::UnexpectedResponse)
        );
        assert!(matches!(
            client
                .handle_publish("$iothub/twin/res/200/?$rid=1&$version=7", b"{}")
                .unwrap(),
            HubEvent::TwinResponse {
                operation: TwinOperation::Get,
                response,
            } if request.matches(response.request_id()) && response.version() == Some(7)
        ));
        assert_eq!(client.snapshot().state, ConnectionState::Online);
    }

    #[test]
    fn reported_properties_use_independent_request_correlation() {
        let mut client = HubClient::new(config(), HubCapabilities::TWINS);
        client.connected(SessionDisposition::Resumed);
        let mut output = [0; 96];
        client.begin_twin_sync(&mut output).unwrap();
        client
            .handle_publish("$iothub/twin/res/200/?$rid=1", b"{}")
            .unwrap();

        let (request, topic) = client.begin_reported_properties(&mut output).unwrap();
        assert_eq!(
            topic.as_str(),
            "$iothub/twin/PATCH/properties/reported/?$rid=2"
        );
        assert_eq!(
            client.handle_publish("$iothub/twin/res/204/?$rid=1", b""),
            Err(HubError::UnexpectedResponse)
        );
        assert!(matches!(
            client
                .handle_publish("$iothub/twin/res/204/?$rid=2&$version=8", b"")
                .unwrap(),
            HubEvent::TwinResponse {
                operation: TwinOperation::ReportedProperties,
                response,
            } if request.matches(response.request_id()) && response.version() == Some(8)
        ));
        assert_eq!(client.snapshot().state, ConnectionState::Online);
    }

    #[test]
    fn routes_enabled_operations_and_records_acceptance_explicitly() {
        let mut client = HubClient::new(config(), all_capabilities());
        let c2d = client
            .handle_publish(
                "devices/sensor-01/messages/devicebound/?command=blink",
                b"on",
            )
            .unwrap();
        assert!(matches!(c2d, HubEvent::CloudToDevice(_)));
        assert_eq!(client.snapshot().inbound_accepted, 0);
        client.accept_inbound(c2d);
        assert_eq!(client.snapshot().inbound_accepted, 1);

        let desired = client
            .handle_publish("$iothub/twin/PATCH/properties/desired/?$version=9", b"{}")
            .unwrap();
        client.accept_inbound(desired);
        assert_eq!(client.desired_version(), Some(9));
    }

    #[test]
    fn disabled_operations_are_not_accidentally_exposed() {
        let mut client = HubClient::new(config(), HubCapabilities::TELEMETRY);
        assert_eq!(
            client.handle_publish(
                "devices/sensor-01/messages/devicebound/?command=blink",
                b"on"
            ),
            Err(HubError::Codec(CodecError::UnexpectedTopic))
        );
        assert!(
            client
                .capabilities()
                .portable()
                .contains(CloudCapabilities::TELEMETRY)
        );
        assert!(
            !client
                .capabilities()
                .portable()
                .contains(CloudCapabilities::COMMANDS)
        );
    }

    #[test]
    fn rejected_twin_sync_is_normalized_and_redacted() {
        let mut client = HubClient::new(config(), HubCapabilities::TWINS);
        client.connected(SessionDisposition::Resumed);
        let mut output = [0; 64];
        client.begin_twin_sync(&mut output).unwrap();
        assert_eq!(
            client.handle_publish("$iothub/twin/res/429/?$rid=1", b"throttled"),
            Err(HubError::ServiceRejected(429))
        );
        assert_eq!(client.snapshot().last_error, Some(ErrorKind::Throttled));
        assert_eq!(client.snapshot().state, ConnectionState::BackingOff);
    }

    #[test]
    fn async_session_correlates_telemetry_acknowledgement() {
        let mut hub = HubClient::new(config(), HubCapabilities::TELEMETRY);
        let mqtt = MockSession::new([]);
        let mut session = HubSession::new(&mut hub, mqtt, SessionDisposition::Fresh).unwrap();
        let mut scratch = [0; 256];

        let operation =
            block_on(session.publish_telemetry(b"{\"temperature\":21}", &mut scratch)).unwrap();
        assert_eq!(
            block_on(session.poll()).unwrap(),
            HubSessionEvent::OutboundAcknowledged {
                operation,
                purpose: OutboundOperation::Telemetry,
            }
        );
        assert_eq!(session.snapshot().outbound_acknowledged, 1);

        let (hub, mqtt) = session.into_parts();
        assert_eq!(hub.snapshot().state, ConnectionState::Online);
        assert_eq!(mqtt.publishes, 1);
    }

    #[test]
    fn async_session_installs_subscriptions_and_resynchronizes_twin() {
        let mut hub = HubClient::new(config(), all_capabilities());
        let mqtt = MockSession::with_twin_response();
        let mut session = HubSession::new(&mut hub, mqtt, SessionDisposition::Fresh).unwrap();
        let mut scratch = [0; 256];

        for expected in [
            SubscriptionKind::CloudToDevice,
            SubscriptionKind::DirectMethods,
            SubscriptionKind::TwinResponses,
            SubscriptionKind::DesiredProperties,
        ] {
            let operation = block_on(session.begin_next_subscription(&mut scratch))
                .unwrap()
                .unwrap();
            assert_eq!(
                block_on(session.poll()).unwrap(),
                HubSessionEvent::SubscriptionAccepted {
                    operation,
                    kind: expected,
                }
            );
        }
        assert!(
            block_on(session.begin_next_subscription(&mut scratch))
                .unwrap()
                .is_none()
        );

        let operation = block_on(session.begin_twin_sync(&mut scratch)).unwrap();
        assert_eq!(
            block_on(session.poll()).unwrap(),
            HubSessionEvent::OutboundAcknowledged {
                operation,
                purpose: OutboundOperation::TwinGet,
            }
        );
        assert!(matches!(
            block_on(session.poll()).unwrap(),
            HubSessionEvent::Inbound(HubEvent::TwinResponse {
                operation: TwinOperation::Get,
                response,
            }) if response.version() == Some(7)
        ));
        block_on(session.accept_inbound()).unwrap();
        assert_eq!(session.snapshot().state, ConnectionState::Online);

        let reported =
            block_on(session.publish_reported_properties(br#"{"active":true}"#, &mut scratch))
                .unwrap();
        assert_eq!(
            block_on(session.poll()).unwrap(),
            HubSessionEvent::OutboundAcknowledged {
                operation: reported,
                purpose: OutboundOperation::ReportedProperties,
            }
        );

        let (_, mqtt) = session.into_parts();
        assert_eq!(mqtt.subscriptions, 4);
        assert_eq!(mqtt.publishes, 2);
        assert_eq!(mqtt.acknowledgements, 0);
    }

    #[test]
    fn async_session_defers_puback_until_application_acceptance() {
        let inbound = SessionEvent::Publish(embedded_sdk_mqtt::InboundPublish::new(
            "devices/sensor-01/messages/devicebound/?command=blink",
            b"on",
            QoS::AtLeastOnce,
            false,
            true,
        ));
        let mut hub = HubClient::new(config(), HubCapabilities::CLOUD_TO_DEVICE);
        let mqtt = MockSession::new([inbound]);
        let mut session = HubSession::new(&mut hub, mqtt, SessionDisposition::Resumed).unwrap();

        assert!(matches!(
            block_on(session.poll()).unwrap(),
            HubSessionEvent::Inbound(HubEvent::CloudToDevice(message))
                if message.payload() == b"on"
        ));
        assert!(matches!(
            block_on(session.poll()),
            Err(HubSessionError::InboundAcceptancePending)
        ));
        assert_eq!(session.snapshot().inbound_accepted, 0);
        block_on(session.accept_inbound()).unwrap();

        let (hub, mqtt) = session.into_parts();
        assert_eq!(hub.snapshot().inbound_accepted, 1);
        assert_eq!(mqtt.acknowledgements, 1);
    }

    #[test]
    fn async_session_copies_and_correlates_direct_method_response() {
        let inbound = SessionEvent::Publish(embedded_sdk_mqtt::InboundPublish::new(
            "$iothub/methods/POST/reboot/?$rid=ab12",
            br#"{"delay":1}"#,
            QoS::AtMostOnce,
            false,
            false,
        ));
        let mut hub = HubClient::new(config(), HubCapabilities::DIRECT_METHODS);
        let mut mqtt = MockSession::new([inbound]);
        mqtt.capabilities = SessionCapabilities::CORRELATED_PUBLISH_ACK
            .union(SessionCapabilities::CORRELATED_SUBSCRIPTION_ACK);
        let mut session = HubSession::new(&mut hub, mqtt, SessionDisposition::Resumed).unwrap();
        let mut scratch = [0; 256];

        let request_id = match block_on(session.poll()).unwrap() {
            HubSessionEvent::Inbound(HubEvent::DirectMethod(request)) => {
                assert_eq!(request.method_name(), "reboot");
                request.owned_request_id().unwrap()
            }
            event => panic!("unexpected event: {event:?}"),
        };
        block_on(session.accept_inbound()).unwrap();

        let operation = block_on(session.respond_direct_method(
            &request_id,
            200,
            br#"{"accepted":true}"#,
            &mut scratch,
        ))
        .unwrap();
        assert_eq!(
            block_on(session.poll()).unwrap(),
            HubSessionEvent::OutboundAcknowledged {
                operation,
                purpose: OutboundOperation::DirectMethodResponse,
            }
        );

        let (hub, mqtt) = session.into_parts();
        assert_eq!(hub.snapshot().inbound_accepted, 1);
        assert_eq!(hub.snapshot().outbound_acknowledged, 1);
        assert_eq!(mqtt.acknowledgements, 0);
        assert_eq!(mqtt.publishes, 1);
    }

    #[test]
    fn async_session_rejects_backend_that_cannot_defer_puback() {
        let mut hub = HubClient::new(config(), HubCapabilities::CLOUD_TO_DEVICE);
        let mut mqtt = MockSession::new([]);
        mqtt.capabilities = SessionCapabilities::CORRELATED_PUBLISH_ACK
            .union(SessionCapabilities::CORRELATED_SUBSCRIPTION_ACK);
        assert!(matches!(
            HubSession::new(&mut hub, mqtt, SessionDisposition::Fresh),
            Err(HubSessionError::UnsupportedBackend)
        ));
    }
}
