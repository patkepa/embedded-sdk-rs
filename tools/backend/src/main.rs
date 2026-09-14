//! Development-only MQTT collector. Started by `cargo xtask telemetry`.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use embedded_sdk_dev_backend::{Store, load_contracts};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, SubscribeFilter};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    healthcheck: bool,
    #[arg(long, default_value = "/contracts")]
    contracts: PathBuf,
    #[arg(long, default_value = "/data/telemetry.sqlite")]
    database: PathBuf,
    #[arg(long, env = "MQTT_HOST", default_value = "mosquitto")]
    mqtt_host: String,
    #[arg(long, env = "MQTT_PORT", default_value_t = 1883)]
    mqtt_port: u16,
    #[arg(long, env = "RETENTION_DAYS", default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=3650))]
    retention_days: u32,
}

fn now_ms() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let ready = std::path::Path::new("/tmp/embedded-sdk-collector-ready");
    if args.healthcheck {
        ensure!(ready.exists(), "MQTT subscription is not ready");
        return Ok(());
    }
    let _ = std::fs::remove_file(ready);
    let contracts = load_contracts(&args.contracts)?;
    let mut store = Store::open(&args.database)?;
    let retention_ms = i64::from(args.retention_days) * 86_400_000;
    store.prune(now_ms()? - retention_ms)?;
    let revision = std::env::var("CONTRACT_REVISION").unwrap_or_else(|_| "local".into());
    let mut options = MqttOptions::new(
        format!("sdk-collector-{revision}"),
        &args.mqtt_host,
        args.mqtt_port,
    );
    options.set_clean_session(false);
    options.set_keep_alive(Duration::from_secs(15));
    options.set_manual_acks(true);
    options.set_max_packet_size(128 * 1024, 128 * 1024);
    let (client, mut eventloop) = AsyncClient::new(options, 100);
    let mut prune = tokio::time::interval(Duration::from_secs(60));
    let mut retry_seconds = 1;
    let shutdown = shutdown();
    tokio::pin!(shutdown);
    eprintln!(
        "Collector ready: {} contract(s), {} day retention",
        contracts.len(),
        args.retention_days
    );
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = prune.tick() => store.prune(now_ms()? - retention_ms)?,
            event = eventloop.poll() => match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    retry_seconds = 1;
                    // One bounded request avoids blocking the event loop on many subscriptions.
                    client.subscribe_many(contracts.iter().map(|contract|
                        SubscribeFilter::new(contract.filter(), QoS::AtLeastOnce))).await?;
                    eprintln!("MQTT connected; subscribing to registered contracts");
                }
                Ok(Event::Incoming(Packet::SubAck(ack))) => {
                    ensure!(ack.return_codes.iter().all(|code| !matches!(code, rumqttc::SubscribeReasonCode::Failure)), "broker rejected subscription");
                    std::fs::write(ready, b"ready")?;
                    eprintln!("MQTT subscriptions ready");
                }
                Ok(Event::Incoming(Packet::Publish(message))) => {
                    // Commit before PUBACK. Storage failure terminates the process so Docker
                    // restarts it and the persistent broker session can redeliver QoS 1.
                    let accepted = store.ingest(&contracts, &message.topic, &message.payload, now_ms()?)
                        .context("persisting MQTT message")?;
                    if !accepted { eprintln!("Message quarantined: contract validation failed"); }
                    client.ack(&message).await?;
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = std::fs::remove_file(ready);
                    eprintln!("MQTT unavailable ({error}); retrying in {retry_seconds}s");
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = tokio::time::sleep(Duration::from_secs(retry_seconds)) => {}
                    }
                    retry_seconds = (retry_seconds * 2).min(30);
                }
            }
        }
    }
    let _ = std::fs::remove_file(ready);
    eprintln!("Collector stopped");
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
