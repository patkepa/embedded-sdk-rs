//! Forward bounded USB scan summaries to the local MQTT telemetry collector.

use std::{
    io::Read,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use rumqttc::{Client, Connection, Event, MqttOptions, Outgoing, Packet, QoS, TryRecvError};
use serde_json::Value;

const PREFIX: &[u8] = b"WIFI_TELEMETRY ";
const MAX_LINE: usize = 2048;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "/dev/cu.usbmodem1201")]
    port: String,
    #[arg(long, default_value = "beetle-01", value_parser = device_id)]
    device: String,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 1883)]
    mqtt_port: u16,
}

fn device_id(value: &str) -> Result<String, String> {
    if (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(value.to_owned())
    } else {
        Err("device must be 1–32 ASCII letters, digits, underscores or hyphens".into())
    }
}

fn payload_from_line(line: &[u8]) -> Option<Vec<u8>> {
    if line.len() > MAX_LINE || !line.starts_with(PREFIX) {
        return None;
    }
    let value: Value = serde_json::from_slice(&line[PREFIX.len()..]).ok()?;
    if !value.is_object()
        || value.get("version")?.as_u64() != Some(1)
        || value.get("kind")?.as_str() != Some("wifi_scan")
    {
        return None;
    }
    serde_json::to_vec(&value).ok()
}

fn recv_event(connection: &mut Connection, timeout: Duration) -> Result<Event> {
    connection
        .recv_timeout(timeout)
        .map_err(|error| anyhow::anyhow!("MQTT event timeout: {error:?}"))?
        .context("MQTT connection failed")
}

fn connect(args: &Args) -> Result<(Client, Connection)> {
    let mut options = MqttOptions::new(
        format!("beetle-bridge-{}", args.device),
        &args.host,
        args.mqtt_port,
    );
    options.set_keep_alive(Duration::from_secs(30));
    let (client, mut connection) = Client::new(options, 4);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "MQTT connection timed out");
        if let Event::Incoming(Packet::ConnAck(ack)) = recv_event(&mut connection, remaining)? {
            ensure!(
                ack.code == rumqttc::ConnectReturnCode::Success,
                "MQTT broker refused connection"
            );
            break;
        }
    }
    Ok((client, connection))
}

fn publish(
    client: &Client,
    connection: &mut Connection,
    topic: &str,
    payload: Vec<u8>,
) -> Result<()> {
    client.publish(topic, QoS::AtLeastOnce, false, payload)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut packet_id = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "MQTT publish was not acknowledged");
        match recv_event(connection, remaining)? {
            Event::Outgoing(Outgoing::Publish(id)) => packet_id = Some(id),
            Event::Incoming(Packet::PubAck(ack)) if packet_id == Some(ack.pkid) => return Ok(()),
            _ => {}
        }
    }
}

fn run_once(args: &Args) -> Result<()> {
    let topic = format!("embedded-sdk/beetle-wifi-scan/v1/{}/telemetry", args.device);
    let (client, mut connection) = connect(args)?;
    eprintln!("MQTT connected to {}:{}", args.host, args.mqtt_port);
    let mut port = serialport::new(&args.port, 115_200)
        .timeout(Duration::from_secs(1))
        .open()
        .with_context(|| format!("opening USB serial port {}", args.port))?;
    eprintln!("reading scanner on {}", args.port);
    let mut line = Vec::with_capacity(512);
    let mut overflow = false;
    let mut buffer = [0_u8; 256];
    loop {
        match port.read(&mut buffer) {
            Ok(count) => {
                for &byte in &buffer[..count] {
                    if byte == b'\n' {
                        if !overflow {
                            let payload =
                                payload_from_line(line.strip_suffix(b"\r").unwrap_or(&line));
                            if let Some(payload) = payload {
                                let value: Value = serde_json::from_slice(&payload)?;
                                publish(&client, &mut connection, &topic, payload)?;
                                eprintln!(
                                    "published channel={} frames={}",
                                    value["channel"], value["frames"]
                                );
                            }
                        }
                        line.clear();
                        overflow = false;
                    } else if !overflow && line.len() < MAX_LINE {
                        line.push(byte);
                    } else {
                        overflow = true;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("reading USB serial port"),
        }
        loop {
            match connection.try_recv() {
                Ok(Ok(Event::Incoming(Packet::ConnAck(_)))) => {}
                Ok(Ok(_)) => {}
                Ok(Err(error)) => bail!("MQTT disconnected: {error}"),
                Err(TryRecvError::Empty) => break,
                Err(error) => bail!("MQTT event loop stopped: {error:?}"),
            }
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    loop {
        if let Err(error) = run_once(&args) {
            eprintln!("bridge disconnected: {error:#}; retrying in 3 seconds");
            std::thread::sleep(Duration::from_secs(3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_bounded_scanner_json() {
        assert!(
            payload_from_line(
                b"WIFI_TELEMETRY {\"version\":1,\"kind\":\"wifi_scan\",\"frames\":4}"
            )
            .is_some()
        );
        assert!(payload_from_line(b"WIFI_TELEMETRY bad json").is_none());
        assert!(
            payload_from_line(b"WIFI_TELEMETRY {\"version\":2,\"kind\":\"wifi_scan\"}").is_none()
        );
        assert!(payload_from_line(&[b'x'; MAX_LINE + 1]).is_none());
    }
}
