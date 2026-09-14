use serde_json::{Value, json};

use crate::Contract;

fn datasource() -> Value {
    json!({"type": "frser-sqlite-datasource", "uid": "embedded-sdk-sqlite"})
}

fn panel(id: usize, title: &str, kind: &str, sql: String, unit: &str) -> Value {
    json!({
        "id": id, "title": title, "type": kind, "datasource": datasource(),
        "gridPos": {"x": ((id - 1) % 2) * 12, "y": ((id - 1) / 2) * 8, "w": 12, "h": 8},
        "fieldConfig": {"defaults": {"unit": unit}, "overrides": []},
        "options": {"legend": {"displayMode": "list", "placement": "bottom"}},
        "targets": [{"refId": "A", "datasource": datasource(), "rawQueryText": sql,
            "queryText": sql, "queryType": if kind == "timeseries" {"time series"} else {"table"},
            "timeColumns": ["time"]}]
    })
}

/// Generate a ready-to-use dashboard with device selection and contract-specific panels.
/// The contract must have passed validation before calling this function.
pub fn dashboard(contract: &Contract) -> Value {
    let scope = format!(
        "contract = '{}' AND device IN (${{device:sqlstring}})",
        contract.id
    );
    let range = "received_ms >= ${__from} AND received_ms < ${__to}";
    let mut panels = vec![
        panel(
            1,
            "Messages per minute",
            "timeseries",
            format!(
                "SELECT (received_ms / 60000) * 60 AS time, device, COUNT(*) AS messages FROM messages WHERE {scope} AND {range} GROUP BY time, device ORDER BY time"
            ),
            "short",
        ),
        panel(
            2,
            "Device last seen (retained history)",
            "table",
            format!(
                "SELECT device, MAX(received_ms) / 1000.0 AS time, COUNT(*) AS messages FROM messages WHERE {scope} GROUP BY device ORDER BY time DESC"
            ),
            "none",
        ),
    ];
    for metric in &contract.metrics {
        panels.push(panel(panels.len() + 1, &metric.title, "timeseries", format!(
            "SELECT (received_ms / MAX(1000, ${{__interval_ms}})) * MAX(1000, ${{__interval_ms}}) / 1000.0 AS time, device, AVG(value) AS value FROM samples WHERE {scope} AND metric = '{}' AND {range} GROUP BY time, device ORDER BY time", metric.key), &metric.unit));
    }
    panels.push(panel(panels.len() + 1, "Recent payloads", "table", format!(
        "SELECT received_ms / 1000.0 AS time, device, payload FROM messages WHERE {scope} AND {range} ORDER BY received_ms DESC LIMIT 100"), "none"));
    panels.push(panel(panels.len() + 1, "Rejected messages (all contracts)", "table", format!(
        "SELECT received_ms / 1000.0 AS time, topic, reason FROM rejected WHERE {range} ORDER BY received_ms DESC LIMIT 100"), "none"));
    json!({
        "uid": format!("sdk-{}", contract.id), "title": contract.title,
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
