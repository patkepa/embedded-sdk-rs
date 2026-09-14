use serde_json::{Value, json};

use crate::Contract;

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

fn messages_per_minute(id: usize, scope: &str, range: &str, scanner: bool) -> Value {
    panel(
        id,
        "Telemetry received per minute",
        if scanner {
            "Accepted MQTT window reports grouped by backend receipt minute. The scanner uploads after a sweep, so bursts and gaps here do not represent Wi-Fi traffic volume."
        } else {
            "Accepted MQTT messages grouped by backend receipt minute. This shows reporting cadence, not device traffic or capture rate."
        },
        "timeseries",
        format!(
            "SELECT (received_ms / 60000) * 60 AS time, device, COUNT(*) AS messages FROM messages WHERE {scope} AND {range} GROUP BY time, device ORDER BY time"
        ),
        "short",
    )
}

fn last_seen(id: usize, scope: &str, scanner: bool) -> Value {
    panel(
        id,
        "Device last seen (retained history)",
        if scanner {
            "Latest accepted report and retained report count per selected scanner. This panel ignores the dashboard time range but is limited by storage retention."
        } else {
            "Latest accepted message and retained message count per selected device. This panel ignores the dashboard time range but is limited by storage retention."
        },
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
    let scanner = contract.id == "beetle-wifi-scan-v1";
    let scope = format!(
        "contract = '{}' AND device IN (${{device:sqlstring}})",
        contract.id
    );
    let range = "received_ms >= ${__from} AND received_ms < ${__to}";
    let mut panels = Vec::new();
    if scanner {
        panels.push(panel(
            1,
            "Recent scan windows",
            "One row per accepted scanner report, newest first. Time is backend receipt time; channel is where the scanner listened, not necessarily the AP's advertised channel. Observed is capture duration in seconds. Frames and data frames count captured traffic; frames/s divides frames by observed time. Retry % is the share of captured data frames with the retry bit, not packet loss. RSSI is average signal at the scanner in dBm. Gap is the preceding blind interval in milliseconds; queue drops count callback frames lost before analysis.",
            "table",
            format!(
                "SELECT received_ms / 1000.0 AS time, device, json_extract(payload, '$.channel') AS channel, ROUND(json_extract(payload, '$.observed_ms') / 1000.0, 1) AS observed_s, json_extract(payload, '$.frames') AS frames, ROUND(json_extract(payload, '$.frames') * 1000.0 / MAX(1, json_extract(payload, '$.observed_ms')), 1) AS frames_per_s, json_extract(payload, '$.data_frames') AS data_frames, json_extract(payload, '$.retry_percent') AS retry_percent, json_extract(payload, '$.rssi_avg_dbm') AS rssi_dbm, json_extract(payload, '$.capture_gap_ms') AS gap_ms, json_extract(payload, '$.queue_dropped') AS queue_dropped FROM messages WHERE {scope} AND {range} ORDER BY received_ms DESC LIMIT 100"
            ),
            "none",
        ));
    } else {
        panels.push(messages_per_minute(1, &scope, range, false));
        panels.push(last_seen(2, &scope, false));
    }
    for metric in &contract.metrics {
        let sql = if scanner {
            format!(
                "SELECT (s.received_ms / MAX(1000, ${{__interval_ms}})) * MAX(1000, ${{__interval_ms}}) / 1000.0 AS time, s.device || ' ch ' || json_extract(m.payload, '$.channel') AS device, AVG(s.value) AS value FROM samples AS s JOIN messages AS m ON m.id = s.message_id WHERE s.contract = '{}' AND s.device IN (${{device:sqlstring}}) AND s.metric = '{}' AND s.received_ms >= ${{__from}} AND s.received_ms < ${{__to}} GROUP BY time, s.device, json_extract(m.payload, '$.channel') ORDER BY time",
                contract.id, metric.key
            )
        } else {
            format!(
                "SELECT (received_ms / MAX(1000, ${{__interval_ms}})) * MAX(1000, ${{__interval_ms}}) / 1000.0 AS time, device, AVG(value) AS value FROM samples WHERE {scope} AND metric = '{}' AND {range} GROUP BY time, device ORDER BY time",
                metric.key
            )
        };
        panels.push(panel(
            panels.len() + 1,
            &metric.title,
            &metric.description,
            "timeseries",
            sql,
            &metric.unit,
        ));
        if scanner && metric.key == "frames" {
            panels.push(panel(
                panels.len() + 1,
                "Captured frames/s",
                "Delivered frames divided by each window's observation duration, then averaged within the Grafana time bucket. Compare the same receive channel; capture can still miss radio traffic.",
                "timeseries",
                format!(
                    "SELECT (s.received_ms / MAX(1000, ${{__interval_ms}})) * MAX(1000, ${{__interval_ms}}) / 1000.0 AS time, s.device || ' ch ' || json_extract(m.payload, '$.channel') AS device, AVG(s.value * 1000.0 / MAX(1, json_extract(m.payload, '$.observed_ms'))) AS value FROM samples AS s JOIN messages AS m ON m.id = s.message_id WHERE s.contract = '{}' AND s.device IN (${{device:sqlstring}}) AND s.metric = 'frames' AND s.received_ms >= ${{__from}} AND s.received_ms < ${{__to}} GROUP BY time, s.device, json_extract(m.payload, '$.channel') ORDER BY time",
                    contract.id
                ),
                "short",
            ));
        }
    }
    if scanner {
        panels.push(messages_per_minute(panels.len() + 1, &scope, range, true));
        panels.push(last_seen(panels.len() + 1, &scope, true));
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
    if scanner {
        for (index, panel) in panels.iter_mut().enumerate() {
            panel["gridPos"] = if index == 0 {
                json!({"x": 0, "y": 0, "w": 24, "h": 8})
            } else {
                json!({"x": ((index - 1) % 2) * 12, "y": 8 + ((index - 1) / 2) * 8, "w": 12, "h": 8})
            };
        }
    }
    json!({
        "uid": format!("sdk-{}", contract.id), "title": contract.title,
        "description": if scanner {
            "Passive 2.4 GHz scan windows. Compare the same receive channel and observation duration; blank signal or retry values mean no sample was available. Counts are observations at the scanner, not device health or packet loss."
        } else {
            "Accepted telemetry and contract metrics from the local development backend."
        },
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
