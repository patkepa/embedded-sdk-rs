use core::fmt;

use crate::{CloudToDeviceMessage, PropertyBag};

/// Largest caller-selected C2D payload or property capacity represented here.
pub const MAX_C2D_FIELD_CAPACITY: usize = u16::MAX as usize;

/// Failure while configuring or operating a bounded C2D queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CloudToDeviceQueueError {
    /// Queue depth or a field capacity was unsupported.
    InvalidConfiguration,
    /// The validated encoded property bag exceeds caller-selected capacity.
    PropertiesTooLong {
        /// Exact required bytes.
        required: usize,
    },
    /// The message payload exceeds caller-selected capacity.
    PayloadTooLong {
        /// Exact required bytes.
        required: usize,
    },
    /// Every compile-time queue slot is occupied.
    Full,
    /// No delivered message is reserved for terminal completion.
    NoActiveMessage,
}

impl fmt::Display for CloudToDeviceQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid cloud-to-device queue configuration")
            }
            Self::PropertiesTooLong { required } => {
                write!(formatter, "C2D properties require {required} bytes")
            }
            Self::PayloadTooLong { required } => {
                write!(formatter, "C2D payload requires {required} bytes")
            }
            Self::Full => formatter.write_str("cloud-to-device queue is full"),
            Self::NoActiveMessage => formatter.write_str("no cloud-to-device message is active"),
        }
    }
}

impl core::error::Error for CloudToDeviceQueueError {}

/// Owned C2D message retained independently of the MQTT receive buffer.
#[derive(Eq, PartialEq)]
pub struct QueuedCloudToDevice<const PROPERTIES: usize, const PAYLOAD: usize> {
    properties: [u8; PROPERTIES],
    properties_len: u16,
    payload: [u8; PAYLOAD],
    payload_len: u16,
}

impl<const PROPERTIES: usize, const PAYLOAD: usize> QueuedCloudToDevice<PROPERTIES, PAYLOAD> {
    fn copy_from(message: CloudToDeviceMessage<'_>) -> Result<Self, CloudToDeviceQueueError> {
        let encoded_properties = message.properties().as_encoded_str();
        if encoded_properties.len() > PROPERTIES {
            return Err(CloudToDeviceQueueError::PropertiesTooLong {
                required: encoded_properties.len(),
            });
        }
        if message.payload().len() > PAYLOAD {
            return Err(CloudToDeviceQueueError::PayloadTooLong {
                required: message.payload().len(),
            });
        }

        let mut properties = [0; PROPERTIES];
        properties[..encoded_properties.len()].copy_from_slice(encoded_properties.as_bytes());
        let mut payload = [0; PAYLOAD];
        payload[..message.payload().len()].copy_from_slice(message.payload());
        Ok(Self {
            properties,
            properties_len: encoded_properties.len() as u16,
            payload,
            payload_len: message.payload().len() as u16,
        })
    }

    /// Returns the validated encoded Azure properties.
    #[must_use]
    pub fn properties(&self) -> PropertyBag<'_> {
        PropertyBag::from_validated(
            core::str::from_utf8(&self.properties[..usize::from(self.properties_len)])
                .unwrap_or(""),
        )
    }

    /// Returns the copied application payload.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_len)]
    }
}

impl<const PROPERTIES: usize, const PAYLOAD: usize> fmt::Debug
    for QueuedCloudToDevice<PROPERTIES, PAYLOAD>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueuedCloudToDevice")
            .field("properties_len", &self.properties_len)
            .field("payload_len", &self.payload_len)
            .finish_non_exhaustive()
    }
}

/// Allocation-free FIFO for application-owned cloud-to-device delivery.
///
/// The front slot remains reserved between `begin_next` and
/// `complete_active`, allowing an application to process the borrowed view
/// without copying the fixed-capacity payload again.
pub struct CloudToDeviceQueue<const DEPTH: usize, const PROPERTIES: usize, const PAYLOAD: usize> {
    slots: [Option<QueuedCloudToDevice<PROPERTIES, PAYLOAD>>; DEPTH],
    head: usize,
    len: usize,
    active: bool,
}

impl<const DEPTH: usize, const PROPERTIES: usize, const PAYLOAD: usize>
    CloudToDeviceQueue<DEPTH, PROPERTIES, PAYLOAD>
{
    /// Creates an empty queue with compile-time resource bounds.
    pub const fn new() -> Result<Self, CloudToDeviceQueueError> {
        if DEPTH == 0 || PROPERTIES > MAX_C2D_FIELD_CAPACITY || PAYLOAD > MAX_C2D_FIELD_CAPACITY {
            return Err(CloudToDeviceQueueError::InvalidConfiguration);
        }
        Ok(Self {
            slots: [const { None }; DEPTH],
            head: 0,
            len: 0,
            active: false,
        })
    }

    /// Copies a validated borrowed message into the next FIFO slot.
    pub fn enqueue(
        &mut self,
        message: CloudToDeviceMessage<'_>,
    ) -> Result<(), CloudToDeviceQueueError> {
        if self.len == DEPTH {
            return Err(CloudToDeviceQueueError::Full);
        }
        let message = QueuedCloudToDevice::copy_from(message)?;
        let tail = (self.head + self.len) % DEPTH;
        self.slots[tail] = Some(message);
        self.len += 1;
        Ok(())
    }

    /// Reserves and borrows the next application message.
    #[must_use]
    pub fn begin_next(&mut self) -> Option<&QueuedCloudToDevice<PROPERTIES, PAYLOAD>> {
        if self.active {
            return None;
        }
        self.active = true;
        let Some(message) = self.slots[self.head].as_ref() else {
            self.active = false;
            return None;
        };
        Some(message)
    }

    /// Releases the active slot after application processing is terminal.
    pub fn complete_active(&mut self) -> Result<(), CloudToDeviceQueueError> {
        if !self.active {
            return Err(CloudToDeviceQueueError::NoActiveMessage);
        }
        self.slots[self.head] = None;
        self.head = (self.head + 1) % DEPTH;
        self.len -= 1;
        self.active = false;
        Ok(())
    }

    /// Returns the occupied slot count, including the active message.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether no message is queued or active.
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
    use crate::{DeviceId, HubConfig, HubHostname, parse_cloud_to_device};

    fn config() -> HubConfig {
        HubConfig::new(
            HubHostname::new("contoso.azure-devices.net").unwrap(),
            DeviceId::new("sensor-01").unwrap(),
            240,
            1024,
        )
        .unwrap()
    }

    fn message<'a>(properties: &'a str, payload: &'a [u8]) -> CloudToDeviceMessage<'a> {
        let mut topic = std::string::String::from("devices/sensor-01/messages/devicebound/");
        topic.push_str(properties);
        let topic = std::boxed::Box::leak(topic.into_boxed_str());
        parse_cloud_to_device(&config(), topic, payload).unwrap()
    }

    #[test]
    fn queue_copies_payload_and_properties_before_receive_buffer_reuse() {
        let mut queue = CloudToDeviceQueue::<2, 32, 16>::new().unwrap();
        queue.enqueue(message("?command=blink", b"first")).unwrap();
        queue.enqueue(message("", b"second")).unwrap();

        let first = queue.begin_next().unwrap();
        assert_eq!(first.payload(), b"first");
        assert_eq!(
            first
                .properties()
                .iter()
                .next()
                .unwrap()
                .unwrap()
                .encoded_name(),
            "command"
        );
        assert_eq!(queue.len(), 2);
        queue.complete_active().unwrap();

        assert_eq!(queue.begin_next().unwrap().payload(), b"second");
        queue.complete_active().unwrap();
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_reports_full_and_exact_field_requirements() {
        let mut queue = CloudToDeviceQueue::<1, 3, 4>::new().unwrap();
        assert_eq!(
            queue.enqueue(message("?name=value", b"ok")),
            Err(CloudToDeviceQueueError::PropertiesTooLong { required: 10 })
        );
        assert_eq!(
            queue.enqueue(message("", b"12345")),
            Err(CloudToDeviceQueueError::PayloadTooLong { required: 5 })
        );
        queue.enqueue(message("", b"1234")).unwrap();
        assert_eq!(
            queue.enqueue(message("", b"next")),
            Err(CloudToDeviceQueueError::Full)
        );
    }

    #[test]
    fn zero_length_fields_are_valid_explicit_bounds() {
        let mut queue = CloudToDeviceQueue::<1, 0, 0>::new().unwrap();
        queue.enqueue(message("", b"")).unwrap();
        assert!(queue.begin_next().unwrap().properties().is_empty());
        assert!(queue.begin_next().is_none());
    }
}
