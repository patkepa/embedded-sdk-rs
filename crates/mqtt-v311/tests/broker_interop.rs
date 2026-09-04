//! Opt-in MQTT 3.1.1 interoperability test against an isolated local broker.

use std::{
    future::Future,
    io::{Read as _, Write as _},
    net::TcpStream,
    task::{Context, Poll, Waker},
    time::Duration,
};

use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use embedded_sdk_mqtt::{
    BrokerHostname, BrokerPort, ClientId, Config, QoS, TopicFilter, TopicName,
};
use embedded_sdk_mqtt_v311::{Buffers, Client, Event, TransportSecurity};

struct BlockingTcp(TcpStream);

impl ErrorType for BlockingTcp {
    type Error = ErrorKind;
}

impl Read for BlockingTcp {
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read(output).map_err(|_| ErrorKind::Other)
    }
}

impl Write for BlockingTcp {
    async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(input).map_err(|_| ErrorKind::Other)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush().map_err(|_| ErrorKind::Other)
    }
}

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

/// Run with a local strict MQTT 3.1.1 broker, for example:
///
/// `MQTT_V311_BROKER_ADDR=127.0.0.1:18883 cargo test -p embedded-sdk-mqtt-v311 --test broker_interop -- --ignored`
#[test]
#[ignore = "requires MQTT_V311_BROKER_ADDR pointing to an isolated plaintext broker"]
fn strict_broker_qos1_round_trip() {
    let address = std::env::var("MQTT_V311_BROKER_ADDR")
        .expect("MQTT_V311_BROKER_ADDR must name the isolated test broker");
    let stream = TcpStream::connect(address).expect("connect to broker fixture");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();

    let client_id = format!("embedded-sdk-interop-{}", std::process::id());
    let config = Config::new_v311(
        BrokerHostname::new("localhost").unwrap(),
        BrokerPort::new(1883).unwrap(),
        ClientId::new(&client_id).unwrap(),
        30,
        false,
        1024,
    )
    .unwrap();
    let mut rx = [0; 1024];
    let mut tx = [0; 1024];
    let mut replay = [0; 1024];
    let mut client = Client::new(
        &config,
        Buffers {
            rx: &mut rx,
            tx: &mut tx,
            replay: &mut replay,
        },
    )
    .unwrap();
    let mut connection = block_on(client.connect(
        BlockingTcp(stream),
        TransportSecurity::PlaintextFixture,
        None,
    ))
    .expect("MQTT 3.1.1 CONNECT");

    let topic = "embedded-sdk/interop/qos1";
    let subscription =
        block_on(connection.subscribe(&TopicFilter::new(topic).unwrap(), QoS::AtLeastOnce))
            .expect("SUBSCRIBE");
    assert!(matches!(
        block_on(connection.poll()).expect("SUBACK"),
        Event::Subscribed { operation, granted_qos: QoS::AtLeastOnce }
            if operation == subscription
    ));

    let publication = block_on(connection.publish(
        &TopicName::new(topic).unwrap(),
        b"mqtt-v311-interoperability",
        QoS::AtLeastOnce,
    ))
    .expect("PUBLISH")
    .expect("QoS 1 operation ID");

    let mut publish_acknowledged = false;
    let mut message_received = false;
    while !publish_acknowledged || !message_received {
        match block_on(connection.poll()).expect("broker event") {
            Event::Published(operation) => {
                assert_eq!(operation, publication);
                publish_acknowledged = true;
            }
            Event::Publish(message) => {
                assert_eq!(message.topic(), topic);
                assert_eq!(message.payload(), b"mqtt-v311-interoperability");
                assert_eq!(message.qos(), QoS::AtLeastOnce);
                assert!(message.acknowledgement_required());
                message_received = true;
                block_on(connection.acknowledge_received()).expect("PUBACK");
            }
            Event::Progress | Event::Subscribed { .. } => {}
        }
    }

    block_on(connection.disconnect()).expect("DISCONNECT");
}
