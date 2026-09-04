use core::fmt;

use crate::{CodecError, DirectMethodRequest, MethodRequestId};

/// Suggested application status when a bounded method queue is full.
pub const DIRECT_METHOD_OVERLOAD_STATUS: u16 = 429;
/// Suggested application status when a direct-method deadline expires.
pub const DIRECT_METHOD_TIMEOUT_STATUS: u16 = 504;
/// Largest caller-selected method name or payload capacity represented here.
pub const MAX_METHOD_FIELD_CAPACITY: usize = u16::MAX as usize;

/// Failure while configuring or operating a bounded direct-method queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DirectMethodQueueError {
    /// Queue depth, field capacity, or timeout was zero or unsupported.
    InvalidConfiguration,
    /// The request identifier cannot be retained for a response.
    RequestId(CodecError),
    /// The encoded method name exceeds the caller-selected capacity.
    MethodNameTooLong {
        /// Exact required bytes.
        required: usize,
    },
    /// The request payload exceeds the caller-selected capacity.
    PayloadTooLong {
        /// Exact required bytes.
        required: usize,
    },
    /// Every compile-time queue slot is occupied.
    Full,
    /// No dispatched request is waiting for terminal completion.
    NoActiveRequest,
}

impl fmt::Display for DirectMethodQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid direct-method queue configuration")
            }
            Self::RequestId(error) => {
                write!(formatter, "invalid direct-method request ID: {error}")
            }
            Self::MethodNameTooLong { required } => {
                write!(formatter, "direct-method name requires {required} bytes")
            }
            Self::PayloadTooLong { required } => {
                write!(formatter, "direct-method payload requires {required} bytes")
            }
            Self::Full => formatter.write_str("direct-method queue is full"),
            Self::NoActiveRequest => formatter.write_str("no direct method is active"),
        }
    }
}

impl core::error::Error for DirectMethodQueueError {}

/// Owned method request retained independently of the MQTT receive buffer.
#[derive(Eq, PartialEq)]
pub struct QueuedDirectMethod<const METHOD_NAME: usize, const PAYLOAD: usize> {
    method_name: [u8; METHOD_NAME],
    method_name_len: u16,
    request_id: MethodRequestId,
    payload: [u8; PAYLOAD],
    payload_len: u16,
    deadline_ms: u64,
}

impl<const METHOD_NAME: usize, const PAYLOAD: usize> QueuedDirectMethod<METHOD_NAME, PAYLOAD> {
    fn copy_from(
        request: DirectMethodRequest<'_>,
        deadline_ms: u64,
    ) -> Result<Self, DirectMethodQueueError> {
        let request_id = request
            .owned_request_id()
            .map_err(DirectMethodQueueError::RequestId)?;
        if request.method_name().len() > METHOD_NAME {
            return Err(DirectMethodQueueError::MethodNameTooLong {
                required: request.method_name().len(),
            });
        }
        if request.payload().len() > PAYLOAD {
            return Err(DirectMethodQueueError::PayloadTooLong {
                required: request.payload().len(),
            });
        }

        let mut method_name = [0; METHOD_NAME];
        method_name[..request.method_name().len()]
            .copy_from_slice(request.method_name().as_bytes());
        let mut payload = [0; PAYLOAD];
        payload[..request.payload().len()].copy_from_slice(request.payload());
        Ok(Self {
            method_name,
            method_name_len: request.method_name().len() as u16,
            request_id,
            payload,
            payload_len: request.payload().len() as u16,
            deadline_ms,
        })
    }

    /// Returns the encoded method name exactly as supplied by IoT Hub.
    #[must_use]
    pub fn method_name(&self) -> &str {
        core::str::from_utf8(&self.method_name[..usize::from(self.method_name_len)]).unwrap_or("")
    }

    /// Returns the owned response correlation identifier.
    #[must_use]
    pub const fn request_id(&self) -> &MethodRequestId {
        &self.request_id
    }

    /// Returns the copied request body.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_len)]
    }

    /// Returns the absolute caller-clock deadline in monotonic milliseconds.
    #[must_use]
    pub const fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }

    /// Returns whether the request deadline has elapsed.
    #[must_use]
    pub const fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.deadline_ms
    }
}

impl<const METHOD_NAME: usize, const PAYLOAD: usize> fmt::Debug
    for QueuedDirectMethod<METHOD_NAME, PAYLOAD>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueuedDirectMethod")
            .field("method_name", &self.method_name())
            .field("request_id", &self.request_id)
            .field("payload_len", &self.payload_len)
            .field("deadline_ms", &self.deadline_ms)
            .finish_non_exhaustive()
    }
}

/// Work returned by [`DirectMethodQueue::begin_next`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectMethodDispatch<'a, const METHOD_NAME: usize, const PAYLOAD: usize> {
    /// Dispatch to the application handler and preserve the slot until completion.
    Ready(&'a QueuedDirectMethod<METHOD_NAME, PAYLOAD>),
    /// Respond with a timeout without invoking the handler.
    TimedOut(&'a QueuedDirectMethod<METHOD_NAME, PAYLOAD>),
}

/// Allocation-free FIFO with compile-time depth and field capacities.
///
/// `begin_next` reserves the front slot as active. The owner must call
/// `complete_active` only after the method response is acknowledged or after a
/// terminal local failure. This makes handler concurrency and queue memory
/// visible in the type.
pub struct DirectMethodQueue<const DEPTH: usize, const METHOD_NAME: usize, const PAYLOAD: usize> {
    slots: [Option<QueuedDirectMethod<METHOD_NAME, PAYLOAD>>; DEPTH],
    head: usize,
    len: usize,
    active: bool,
    timeout_ms: u32,
}

impl<const DEPTH: usize, const METHOD_NAME: usize, const PAYLOAD: usize>
    DirectMethodQueue<DEPTH, METHOD_NAME, PAYLOAD>
{
    /// Creates an empty queue with one deadline policy for all requests.
    pub const fn new(timeout_ms: u32) -> Result<Self, DirectMethodQueueError> {
        if DEPTH == 0
            || METHOD_NAME == 0
            || METHOD_NAME > MAX_METHOD_FIELD_CAPACITY
            || PAYLOAD > MAX_METHOD_FIELD_CAPACITY
            || timeout_ms == 0
        {
            return Err(DirectMethodQueueError::InvalidConfiguration);
        }
        Ok(Self {
            slots: [const { None }; DEPTH],
            head: 0,
            len: 0,
            active: false,
            timeout_ms,
        })
    }

    /// Copies a borrowed request into the next FIFO slot.
    pub fn enqueue(
        &mut self,
        request: DirectMethodRequest<'_>,
        now_ms: u64,
    ) -> Result<(), DirectMethodQueueError> {
        if self.len == DEPTH {
            return Err(DirectMethodQueueError::Full);
        }
        let deadline_ms = now_ms.saturating_add(u64::from(self.timeout_ms));
        let request = QueuedDirectMethod::copy_from(request, deadline_ms)?;
        let tail = (self.head + self.len) % DEPTH;
        self.slots[tail] = Some(request);
        self.len += 1;
        Ok(())
    }

    /// Reserves the front request and classifies its current deadline state.
    #[must_use]
    pub fn begin_next(
        &mut self,
        now_ms: u64,
    ) -> Option<DirectMethodDispatch<'_, METHOD_NAME, PAYLOAD>> {
        if self.active {
            return None;
        }
        self.active = true;
        let Some(request) = self.slots[self.head].as_ref() else {
            self.active = false;
            return None;
        };
        Some(if request.is_expired(now_ms) {
            DirectMethodDispatch::TimedOut(request)
        } else {
            DirectMethodDispatch::Ready(request)
        })
    }

    /// Releases the active front slot after terminal response handling.
    pub fn complete_active(&mut self) -> Result<(), DirectMethodQueueError> {
        if !self.active {
            return Err(DirectMethodQueueError::NoActiveRequest);
        }
        self.slots[self.head] = None;
        self.head = (self.head + 1) % DEPTH;
        self.len -= 1;
        self.active = false;
        Ok(())
    }

    /// Returns the active request, if any, for deadline checks while handling.
    #[must_use]
    pub fn active(&self) -> Option<&QueuedDirectMethod<METHOD_NAME, PAYLOAD>> {
        self.active
            .then(|| self.slots[self.head].as_ref())
            .flatten()
    }

    /// Returns the occupied slot count, including the active request.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether no request is queued or active.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_direct_method;

    fn request<'a>(id: &'a str, payload: &'a [u8]) -> DirectMethodRequest<'a> {
        let mut topic = std::string::String::from("$iothub/methods/POST/reboot/?$rid=");
        topic.push_str(id);
        let topic = std::boxed::Box::leak(topic.into_boxed_str());
        parse_direct_method(topic, payload).unwrap()
    }

    #[test]
    fn queue_is_fifo_and_keeps_active_slot_reserved() {
        let mut queue = DirectMethodQueue::<2, 8, 16>::new(1_000).unwrap();
        queue.enqueue(request("1", b"first"), 100).unwrap();
        queue.enqueue(request("2", b"second"), 200).unwrap();
        assert_eq!(
            queue.enqueue(request("3", b"third"), 300),
            Err(DirectMethodQueueError::Full)
        );

        let DirectMethodDispatch::Ready(first) = queue.begin_next(500).unwrap() else {
            panic!("request unexpectedly timed out");
        };
        assert_eq!(first.request_id().as_str(), "1");
        assert_eq!(first.payload(), b"first");
        assert!(queue.begin_next(500).is_none());
        assert_eq!(queue.len(), 2);

        queue.complete_active().unwrap();
        let DirectMethodDispatch::Ready(second) = queue.begin_next(500).unwrap() else {
            panic!("request unexpectedly timed out");
        };
        assert_eq!(second.request_id().as_str(), "2");
    }

    #[test]
    fn queued_and_active_deadlines_are_explicit() {
        let mut queue = DirectMethodQueue::<1, 8, 16>::new(1_000).unwrap();
        queue.enqueue(request("ab", b"{}"), 10).unwrap();
        let DirectMethodDispatch::TimedOut(expired) = queue.begin_next(1_010).unwrap() else {
            panic!("request should be expired");
        };
        assert_eq!(expired.deadline_ms(), 1_010);
        assert!(queue.active().unwrap().is_expired(1_010));
        queue.complete_active().unwrap();
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_rejects_invalid_configuration_and_field_overflow() {
        assert!(matches!(
            DirectMethodQueue::<0, 8, 16>::new(1_000),
            Err(DirectMethodQueueError::InvalidConfiguration)
        ));
        let mut queue = DirectMethodQueue::<1, 3, 2>::new(1_000).unwrap();
        assert_eq!(
            queue.enqueue(request("1", b"{}"), 0),
            Err(DirectMethodQueueError::MethodNameTooLong { required: 6 })
        );

        let mut queue = DirectMethodQueue::<1, 8, 1>::new(1_000).unwrap();
        assert_eq!(
            queue.enqueue(request("1", b"{}"), 0),
            Err(DirectMethodQueueError::PayloadTooLong { required: 2 })
        );
    }
}
