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

#[test]
fn scanner_dashboard_keeps_batched_channels_separate_and_explains_panels() {
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
    for (channel, frames) in [(1, 100), (6, 200)] {
        let mut payload = contract.example.clone();
        payload["channel"] = json!(channel);
        payload["frames"] = json!(frames);
        assert!(
            store
                .ingest(
                    &[contract.clone()],
                    "embedded-sdk/beetle-wifi-scan/v1/scanner-a/telemetry",
                    &serde_json::to_vec(&payload).unwrap(),
                    5_000,
                )
                .unwrap()
        );
    }
    let dashboard = dashboard(&contract);
    let panels = dashboard["panels"].as_array().unwrap();
    assert_eq!(panels[0]["title"], "Recent scan windows");
    assert_eq!(panels[0]["gridPos"]["w"], 24);
    assert!(
        panels
            .iter()
            .all(|panel| panel["description"].as_str().is_some_and(|s| !s.is_empty()))
    );
    let reader = Connection::open(&path).unwrap();
    for panel in panels {
        let query = panel["targets"][0]["rawQueryText"]
            .as_str()
            .unwrap()
            .replace("${device:sqlstring}", "'scanner-a'")
            .replace("${__from}", "0")
            .replace("${__to}", "10000")
            .replace("${__interval_ms}", "1000");
        let mut statement = reader.prepare(&query).unwrap();
        statement.query([]).unwrap().next().unwrap();
    }
    let frames_panel = panels
        .iter()
        .find(|panel| panel["title"] == "Captured frames per window")
        .unwrap();
    let query = frames_panel["targets"][0]["rawQueryText"]
        .as_str()
        .unwrap()
        .replace("${device:sqlstring}", "'scanner-a'")
        .replace("${__from}", "0")
        .replace("${__to}", "10000")
        .replace("${__interval_ms}", "1000");
    let series: Vec<(String, f64)> = reader
        .prepare(&query)
        .unwrap()
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        series,
        [
            ("scanner-a ch 1".into(), 100.0),
            ("scanner-a ch 6".into(), 200.0)
        ]
    );
    let rate_panel = panels
        .iter()
        .find(|panel| panel["title"] == "Captured frames/s")
        .unwrap();
    let query = rate_panel["targets"][0]["rawQueryText"]
        .as_str()
        .unwrap()
        .replace("${device:sqlstring}", "'scanner-a'")
        .replace("${__from}", "0")
        .replace("${__to}", "10000")
        .replace("${__interval_ms}", "1000");
    let rates: Vec<f64> = reader
        .prepare(&query)
        .unwrap()
        .query_map([], |row| row.get(2))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rates, [20.0, 40.0]);
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
