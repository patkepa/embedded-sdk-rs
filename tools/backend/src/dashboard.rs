use serde_json::{Value, json};

use crate::Contract;

mod scanner;

fn datasource() -> Value {
    json!({"type": "frser-sqlite-datasource", "uid": "embedded-sdk-sqlite"})
}

fn panel(id: usize, title: &str, description: &str, kind: &str, sql: String, unit: &str) -> Value {
    json!({
        "id": id, "title": title, "description": description,
        "type": kind, "datasource": datasource(),
        "gridPos": {"x": ((id - 1) % 2) * 12, "y": ((id - 1) / 2) * 8, "w": 12, "h": 8},
        "fieldConfig": {"defaults": {"unit": unit}, "overrides": []},
        "options": {"legend": {"displayMode": "list", "placement": "bottom"}},
        "targets": [{"refId": "A", "datasource": datasource(), "rawQueryText": sql,
            "queryText": sql, "queryType": if kind == "timeseries" {"time series"} else {"table"},
            "timeColumns": ["time"]}]
    })
}

fn messages_per_minute(id: usize, scope: &str, range: &str) -> Value {
    panel(
        id,
        "Telemetry received per minute",
        "Accepted MQTT messages grouped by backend receipt minute. This shows reporting cadence, not device traffic or capture rate.",
        "timeseries",
        format!(
            "SELECT (received_ms / 60000) * 60 AS time, device, COUNT(*) AS messages FROM messages WHERE {scope} AND {range} GROUP BY time, device ORDER BY time"
        ),
        "short",
    )
}

fn last_seen(id: usize, scope: &str) -> Value {
    panel(
        id,
        "Device last seen (retained history)",
        "Latest accepted message and retained message count per selected device. This panel ignores the dashboard time range but is limited by storage retention.",
        "table",
        format!(
            "SELECT device, MAX(received_ms) / 1000.0 AS time, COUNT(*) AS messages FROM messages WHERE {scope} GROUP BY device ORDER BY time DESC"
        ),
        "none",
    )
}

/// Generate a ready-to-use dashboard with device selection and contract-specific panels.
/// The contract must have passed validation before calling this function.
pub fn dashboard(contract: &Contract) -> Value {
    if contract.id == "beetle-wifi-scan-v1" {
        return scanner::dashboard(contract);
    }
    let scope = format!(
        "contract = '{}' AND device IN (${{device:sqlstring}})",
        contract.id
    );
    let range = "received_ms >= ${__from} AND received_ms < ${__to}";
    let mut panels = Vec::new();
    panels.push(messages_per_minute(1, &scope, range));
    panels.push(last_seen(2, &scope));
    for metric in &contract.metrics {
        let sql = format!(
            "SELECT (received_ms / MAX(1000, ${{__interval_ms}})) * MAX(1000, ${{__interval_ms}}) / 1000.0 AS time, device, AVG(value) AS value FROM samples WHERE {scope} AND metric = '{}' AND {range} GROUP BY time, device ORDER BY time",
            metric.key
        );
        panels.push(panel(
            panels.len() + 1,
            &metric.title,
            &metric.description,
            "timeseries",
            sql,
            &metric.unit,
        ));
    }
    panels.push(panel(
        panels.len() + 1,
        "Recent payloads",
        "Raw accepted JSON messages in the selected time range, newest first. These are backend receipt times, not device timestamps.",
        "table",
        format!(
            "SELECT received_ms / 1000.0 AS time, device, payload FROM messages WHERE {scope} AND {range} ORDER BY received_ms DESC LIMIT 100"
        ),
        "none",
    ));
    panels.push(panel(
        panels.len() + 1,
        "Rejected messages (all contracts)",
        "Recent payloads rejected by contract validation, including their MQTT topic and reason. This is a backend-wide diagnostic and ignores the device selector.",
        "table",
        format!(
            "SELECT received_ms / 1000.0 AS time, topic, reason FROM rejected WHERE {range} ORDER BY received_ms DESC LIMIT 100"
        ),
        "none",
    ));
    json!({
        "uid": format!("sdk-{}", contract.id), "title": contract.title,
        "description": "Accepted telemetry and contract metrics from the local development backend.",
        "schemaVersion": 39, "version": 1, "editable": false,
        "tags": ["embedded-sdk", contract.id], "timezone": "browser",
        "refresh": "5s", "time": {"from": "now-1h", "to": "now"},
        "templating": {"list": [{"name": "device", "label": "Device", "type": "query",
            "datasource": datasource(), "query": format!("SELECT DISTINCT device FROM messages WHERE contract = '{}' ORDER BY device", contract.id),
            "refresh": 2, "multi": true, "includeAll": true,
            "current": {"text": "All", "value": "$__all"}, "options": []}]},
        "panels": panels
    })
}
