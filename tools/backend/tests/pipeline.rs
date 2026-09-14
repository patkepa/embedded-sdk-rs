//! Contract, storage, retention, and generated-query integration tests.

use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use embedded_sdk_dev_backend::{Contract, Metric, Store, dashboard, validate_contracts};
use rusqlite::Connection;
use serde_json::json;

fn contract() -> Contract {
    Contract {
        id: "sensor-v1".into(),
        title: "Development sensor".into(),
        version: 1,
        topic: "sdk/sensors/{device}/telemetry".into(),
        equals: BTreeMap::new(),
        metrics: vec![Metric {
            key: "temperature".into(),
            title: "Temperature".into(),
            description: "Ambient temperature at the sensor".into(),
            pointer: "/readings/temperature".into(),
            unit: "celsius".into(),
            required: true,
        }],
        example: json!({"version": 1, "readings": {"temperature": 23.5}}),
    }
}

#[test]
fn firmware_example_matches_the_actual_wire_payload() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../firmware/seeed/xiao-esp32c6/telemetry.json"
    ))
    .unwrap();
    contract.validate().unwrap();
    let firmware = include_str!("../../../firmware/seeed/xiao-esp32c6/src/main.rs");
    assert!(
        firmware.contains(&format!(
            "br#\"{}\"#",
            serde_json::to_string(&contract.example).unwrap()
        )) || firmware.contains("br#\"{\"version\":1,\"kind\":\"heartbeat\"}\"#")
    );
    assert_eq!(contract.example, json!({"version": 1, "kind": "heartbeat"}));
}

#[test]
fn routes_are_exact_and_ambiguous_contracts_fail() {
    let a = contract();
    assert_eq!(a.device("sdk/sensors/a/telemetry"), Some("a"));
    for topic in [
        "sdk/sensors//telemetry",
        "sdk/sensors/a/commands",
        "sdk/sensors/a/telemetry/extra",
    ] {
        assert_eq!(a.device(topic), None);
    }
    let mut b = a.clone();
    b.id = "another".into();
    b.topic = "sdk/{device}/a/telemetry".into();
    assert!(validate_contracts(&[a, b]).is_err());
}

#[test]
fn invalid_contracts_fail_before_startup() {
    let mut c = contract();
    c.metrics[0].pointer = "readings/temperature".into();
    assert!(c.validate().is_err());
    c = contract();
    c.metrics.push(c.metrics[0].clone());
    assert!(c.validate().is_err());
    c = contract();
    c.example["version"] = json!(2);
    assert!(c.validate().is_err());
    c = contract();
    c.id = "quote'injection".into();
    assert!(c.validate().is_err());
    assert!(validate_contracts(&[]).is_err());
}

#[test]
fn collection_is_atomic_persistent_and_queries_work() {
    let path = std::env::temp_dir().join(format!(
        "sdk-backend-test-{}-{}.sqlite",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let c = contract();
    let contracts = [c.clone()];
    {
        let mut store = Store::open(&path).unwrap();
        assert!(
            store
                .ingest(
                    &contracts,
                    "sdk/sensors/room'1/telemetry",
                    &serde_json::to_vec(&c.example).unwrap(),
                    1_000
                )
                .unwrap()
        );
        assert!(
            store
                .ingest(
                    &contracts,
                    "sdk/sensors/room'1/telemetry",
                    &serde_json::to_vec(&c.example).unwrap(),
                    2_000
                )
                .unwrap()
        );
        for invalid in [
            b"not-json".as_slice(),
            br#"{"version":2}"#,
            br#"{"version":1}"#,
            br#"{"version":1,"readings":{"temperature":"hot"}}"#,
        ] {
            assert!(
                !store
                    .ingest(&contracts, "sdk/sensors/room'1/telemetry", invalid, 1_000)
                    .unwrap()
            );
        }
        let reader =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT COUNT(*) FROM samples", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            reader
                .query_row("SELECT COUNT(*) FROM rejected", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            4
        );
        // Execute the actual generated queries, including quoting a device containing an apostrophe.
        let dashboard = dashboard(&c);
        for panel in dashboard["panels"].as_array().unwrap() {
            let query = panel["targets"][0]["rawQueryText"]
                .as_str()
                .unwrap()
                .replace("${device:sqlstring}", "'room''1'")
                .replace("${__from}", "0")
                .replace("${__to}", "10000")
                .replace("${__interval_ms}", "1000");
            let mut statement = reader.prepare(&query).unwrap();
            assert!(
                statement.query([]).unwrap().next().unwrap().is_some(),
                "{query}"
            );
        }
        drop(reader);
        store.prune(1_500).unwrap();
    }
    let reader = Connection::open(&path).unwrap();
    for table in ["messages", "samples"] {
        assert_eq!(
            reader
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    assert_eq!(
        reader
            .query_row("SELECT COUNT(*) FROM rejected", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(reader);
    std::fs::remove_file(path).unwrap();
}

fn scanner_query(panel: &serde_json::Value) -> String {
    panel["targets"][0]["rawQueryText"]
        .as_str()
        .unwrap()
        .replace("${device:sqlstring}", "'scanner-a'")
        .replace("${channel:csv}", "1,6")
        .replace("${__from}", "0")
        .replace("${__to}", "100000")
        .replace("${__interval_ms}", "1000")
}

#[test]
fn scanner_dashboard_uses_capture_time_and_weighted_channel_metrics() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../firmware/dfrobot/beetle-esp32c6-wifi-scanner/telemetry.json"
    ))
    .unwrap();
    contract.validate().unwrap();
    let path = std::env::temp_dir().join(format!(
        "sdk-scanner-dashboard-{}-{}.sqlite",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut store = Store::open(&path).unwrap();
    for (channel, frames, observed_ms, data_frames, data_retries, received_ms) in [
        (1, 100, 5_000, 20, 2, 70_000),
        (6, 200, 5_000, 20, 2, 70_000),
        (6, 100, 10_000, 10, 5, 70_000),
    ] {
        let mut payload = contract.example.clone();
        payload["channel"] = json!(channel);
        payload["frames"] = json!(frames);
        payload["observed_ms"] = json!(observed_ms);
        payload["data_frames"] = json!(data_frames);
        payload["data_retries"] = json!(data_retries);
        payload["upload_uptime_ms"] = json!(65_000);
        assert!(
            store
                .ingest(
                    &[contract.clone()],
                    "embedded-sdk/beetle-wifi-scan/v1/scanner-a/telemetry",
                    &serde_json::to_vec(&payload).unwrap(),
                    received_ms,
                )
                .unwrap()
        );
    }
    let dashboard = dashboard(&contract);
    let panels = dashboard["panels"].as_array().unwrap();
    assert_eq!(panels.len(), 11);
    assert_eq!(panels[0]["type"], "stat");
    assert_eq!(panels[3]["title"], "Latest window on each receive channel");
    assert_eq!(panels[3]["gridPos"]["w"], 24);
    assert_eq!(panels.last().unwrap()["type"], "row");
    assert_eq!(panels.last().unwrap()["collapsed"], true);
    assert!(
        panels
            .iter()
            .all(|panel| panel["description"].as_str().is_some_and(|s| !s.is_empty()))
    );
    let reader = Connection::open(&path).unwrap();
    for panel in panels
        .iter()
        .chain(panels.last().unwrap()["panels"].as_array().unwrap())
    {
        if panel["type"] == "row" {
            continue;
        }
        let query = scanner_query(panel);
        let mut statement = reader.prepare(&query).unwrap();
        statement.query([]).unwrap().next().unwrap();
    }
    let overview = panels
        .iter()
        .find(|panel| panel["title"] == "Latest window on each receive channel")
        .unwrap();
    let overview_rows: Vec<(i64, f64, String)> = reader
        .prepare(&scanner_query(overview))
        .unwrap()
        .query_map([], |row| Ok((row.get(2)?, row.get(3)?, row.get(12)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        overview_rows,
        [
            (1, 20.0, "estimated capture".into()),
            (6, 10.0, "estimated capture".into())
        ]
    );
    let rate_panel = panels
        .iter()
        .find(|panel| panel["title"] == "Captured frames/s")
        .unwrap();
    let rates: Vec<(f64, String, f64)> = reader
        .prepare(&scanner_query(rate_panel))
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rates,
        [
            (10.0, "scanner-a ch 1".into(), 20.0),
            (10.0, "scanner-a ch 6".into(), 20.0)
        ]
    );
    let retry_panel = panels
        .iter()
        .find(|panel| panel["title"] == "Observed data retry share")
        .unwrap();
    let retries: Vec<(String, f64)> = reader
        .prepare(&scanner_query(retry_panel))
        .unwrap()
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(retries.len(), 2);
    assert!((retries[1].1 - 23.333333333).abs() < 0.0001);
    let windows = &panels.last().unwrap()["panels"][0];
    let capture_times: Vec<f64> = reader
        .prepare(&scanner_query(windows))
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(capture_times, [10.0, 10.0, 10.0]);
    drop(reader);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn optional_absent_fields_are_allowed_but_invalid_values_are_not() {
    let mut c = contract();
    c.metrics[0].required = false;
    c.example = json!({"version": 1});
    c.validate().unwrap();
    let mut store = Store::open(Path::new(":memory:")).unwrap();
    assert!(
        store
            .ingest(
                &[c.clone()],
                "sdk/sensors/a/telemetry",
                br#"{"version":1}"#,
                0
            )
            .unwrap()
    );
    assert!(
        !store
            .ingest(
                &[c],
                "sdk/sensors/a/telemetry",
                br#"{"version":1,"readings":{"temperature":null}}"#,
                0
            )
            .unwrap()
    );
}
