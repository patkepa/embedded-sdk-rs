use core::fmt;

use embedded_sdk_mqtt::TopicName;

/// Largest caller-selected telemetry payload capacity represented here.
pub const MAX_TELEMETRY_PAYLOAD_CAPACITY: usize = u16::MAX as usize;

/// Local non-zero token used to report terminal telemetry outcomes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TelemetryToken(u32);

impl TelemetryToken {
    /// Returns the local correlation value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Failure while configuring or operating a bounded telemetry queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TelemetryQueueError {
    /// Queue depth or payload capacity was unsupported.
    InvalidConfiguration,
    /// The telemetry payload exceeds caller-selected capacity.
    PayloadTooLong {
        /// Exact required bytes.
        required: usize,
    },
    /// Every compile-time queue slot is occupied.
    Full,
    /// No published or expired telemetry entry is active.
    NoActiveEntry,
}

impl fmt::Display for TelemetryQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid telemetry queue configuration")
            }
            Self::PayloadTooLong { required } => {
                write!(formatter, "telemetry payload requires {required} bytes")
            }
            Self::Full => formatter.write_str("telemetry queue is full"),
            Self::NoActiveEntry => formatter.write_str("no telemetry entry is active"),
        }
    }
}

impl core::error::Error for TelemetryQueueError {}

/// Owned telemetry publication retained independently of producer storage.
#[derive(Eq, PartialEq)]
pub struct QueuedTelemetry<const PAYLOAD: usize> {
    token: TelemetryToken,
    topic: TopicName,
    payload: [u8; PAYLOAD],
    payload_len: u16,
    expires_at_ms: Option<u64>,
}

impl<const PAYLOAD: usize> QueuedTelemetry<PAYLOAD> {
    /// Returns the local completion-correlation token.
    #[must_use]
    pub const fn token(&self) -> TelemetryToken {
        self.token
    }

    /// Returns the complete prevalidated MQTT topic, including Azure properties.
    #[must_use]
    pub const fn topic(&self) -> &TopicName {
        &self.topic
    }

    /// Returns the copied telemetry payload.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_len)]
    }

    /// Returns the optional absolute monotonic expiration deadline.
    #[must_use]
    pub const fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }

    /// Returns whether an unpublished entry is stale at the supplied caller time.
    #[must_use]
    pub const fn is_expired(&self, now_ms: u64) -> bool {
        matches!(self.expires_at_ms, Some(deadline) if now_ms >= deadline)
    }
}

impl<const PAYLOAD: usize> fmt::Debug for QueuedTelemetry<PAYLOAD> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueuedTelemetry")
            .field("token", &self.token)
            .field("topic_len", &self.topic.len())
            .field("payload_len", &self.payload_len)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

/// Work returned by [`TelemetryQueue::begin_next`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryDispatch<'a, const PAYLOAD: usize> {
    /// Publish this entry and retain its slot until correlated PUBACK.
    Ready(&'a QueuedTelemetry<PAYLOAD>),
    /// Drop this stale unpublished entry and report its local token.
    Expired(&'a QueuedTelemetry<PAYLOAD>),
}

/// Allocation-free FIFO for RAM-retained telemetry.
///
/// The front slot remains active until `complete_active` is called. When a
/// transport fails after publish, the owner retains this queue and reattaches
/// the replaying MQTT backend with `HubSession::recover_telemetry`; it releases
/// the slot only after the recovered PUBACK. Entries do not survive reset or
/// power loss and therefore do not provide durable delivery.
pub struct TelemetryQueue<const DEPTH: usize, const PAYLOAD: usize> {
    slots: [Option<QueuedTelemetry<PAYLOAD>>; DEPTH],
    head: usize,
    len: usize,
    active: bool,
    next_token: u32,
}

impl<const DEPTH: usize, const PAYLOAD: usize> TelemetryQueue<DEPTH, PAYLOAD> {
    /// Creates an empty queue with compile-time memory bounds.
    pub const fn new() -> Result<Self, TelemetryQueueError> {
        if DEPTH == 0 || PAYLOAD > MAX_TELEMETRY_PAYLOAD_CAPACITY {
            return Err(TelemetryQueueError::InvalidConfiguration);
        }
        Ok(Self {
            slots: [const { None }; DEPTH],
            head: 0,
            len: 0,
            active: false,
            next_token: 1,
        })
    }

    /// Copies one publication into the next FIFO slot.
    pub fn enqueue(
        &mut self,
        topic: &TopicName,
        payload: &[u8],
        expires_at_ms: Option<u64>,
    ) -> Result<TelemetryToken, TelemetryQueueError> {
        if self.len == DEPTH {
            return Err(TelemetryQueueError::Full);
        }
        if payload.len() > PAYLOAD {
            return Err(TelemetryQueueError::PayloadTooLong {
                required: payload.len(),
            });
        }
        let token = TelemetryToken(self.next_token);
        self.next_token = self.next_token.wrapping_add(1);
        if self.next_token == 0 {
            self.next_token = 1;
        }
        let mut owned_payload = [0; PAYLOAD];
        owned_payload[..payload.len()].copy_from_slice(payload);
        let tail = (self.head + self.len) % DEPTH;
        self.slots[tail] = Some(QueuedTelemetry {
            token,
            topic: *topic,
            payload: owned_payload,
            payload_len: payload.len() as u16,
            expires_at_ms,
        });
        self.len += 1;
        Ok(token)
    }

    /// Reserves the front entry and classifies unpublished expiration.
    #[must_use]
    pub fn begin_next(&mut self, now_ms: u64) -> Option<TelemetryDispatch<'_, PAYLOAD>> {
        if self.active {
            return None;
        }
        self.active = true;
        let Some(entry) = self.slots[self.head].as_ref() else {
            self.active = false;
            return None;
        };
        Some(if entry.is_expired(now_ms) {
            TelemetryDispatch::Expired(entry)
        } else {
            TelemetryDispatch::Ready(entry)
        })
    }

    /// Returns the active entry retained across publish and reconnect attempts.
    #[must_use]
    pub fn active(&self) -> Option<&QueuedTelemetry<PAYLOAD>> {
        if self.active {
            self.slots[self.head].as_ref()
        } else {
            None
        }
    }

    /// Releases the active entry after PUBACK or an explicit terminal drop.
    pub fn complete_active(&mut self) -> Result<TelemetryToken, TelemetryQueueError> {
        if !self.active {
            return Err(TelemetryQueueError::NoActiveEntry);
        }
        let token = self.slots[self.head]
            .as_ref()
            .ok_or(TelemetryQueueError::NoActiveEntry)?
            .token;
        self.slots[self.head] = None;
        self.head = (self.head + 1) % DEPTH;
        self.len -= 1;
        self.active = false;
        Ok(token)
    }

    /// Returns occupied slots, including an in-flight publication.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether no telemetry is queued or in flight.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the compile-time queue depth.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        DEPTH
    }
}

impl<'a, const PAYLOAD: usize> TelemetryDispatch<'a, PAYLOAD> {
    /// Returns the retained queue entry for either dispatch outcome.
    #[must_use]
    pub const fn entry(self) -> &'a QueuedTelemetry<PAYLOAD> {
        match self {
            Self::Ready(entry) | Self::Expired(entry) => entry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic() -> TopicName {
        TopicName::new("devices/sensor/messages/events/$.ct=application%2Fjson").unwrap()
    }

    #[test]
    fn active_entry_is_retained_until_terminal_completion() {
        let mut queue = TelemetryQueue::<2, 16>::new().unwrap();
        let first = queue.enqueue(&topic(), b"first", None).unwrap();
        let second = queue.enqueue(&topic(), b"second", None).unwrap();
        assert_ne!(first, second);
        let Some(TelemetryDispatch::Ready(active)) = queue.begin_next(0) else {
            panic!("expected ready telemetry");
        };
        assert_eq!(active.token(), first);
        assert_eq!(active.payload(), b"first");
        assert!(queue.begin_next(0).is_none());
        assert_eq!(queue.complete_active().unwrap(), first);
        assert_eq!(queue.begin_next(0).unwrap().entry().token(), second);
    }

    #[test]
    fn expiration_and_capacity_failures_are_explicit() {
        let mut queue = TelemetryQueue::<1, 4>::new().unwrap();
        assert_eq!(
            queue.enqueue(&topic(), b"12345", None),
            Err(TelemetryQueueError::PayloadTooLong { required: 5 })
        );
        let token = queue.enqueue(&topic(), b"1234", Some(10)).unwrap();
        assert_eq!(
            queue.enqueue(&topic(), b"next", None),
            Err(TelemetryQueueError::Full)
        );
        let Some(TelemetryDispatch::Expired(expired)) = queue.begin_next(10) else {
            panic!("expected expired telemetry");
        };
        assert_eq!(expired.token(), token);
        assert_eq!(queue.complete_active().unwrap(), token);
    }
}
