use serde_json::{Value, json};

use super::{datasource, panel};
use crate::Contract;

const CAPTURE_RANGE: &str = "capture_ms >= ${__from} AND capture_ms < ${__to}";
const CHANNEL_FILTER: &str = "channel IN (${channel:csv})";
const BUCKET: &str =
    "(capture_ms / MAX(1000, ${__interval_ms})) * MAX(1000, ${__interval_ms}) / 1000.0";

fn scan_cte(scope: &str) -> String {
    // upload_uptime_ms is absent from reports produced by older firmware. Those
    // windows fall back to receipt time, which the table makes explicit.
    format!(
        r#"WITH scan AS (
            SELECT id, device, received_ms,
                CASE WHEN json_type(payload, '$.upload_uptime_ms') = 'integer'
                    AND json_type(payload, '$.uptime_ms') = 'integer'
                    AND json_extract(payload, '$.upload_uptime_ms') >= json_extract(payload, '$.uptime_ms')
                    AND json_extract(payload, '$.upload_uptime_ms') - json_extract(payload, '$.uptime_ms') <= 300000
                    THEN received_ms - (json_extract(payload, '$.upload_uptime_ms') - json_extract(payload, '$.uptime_ms'))
                    ELSE received_ms END AS capture_ms,
                CASE WHEN json_type(payload, '$.upload_uptime_ms') = 'integer'
                    AND json_type(payload, '$.uptime_ms') = 'integer'
                    AND json_extract(payload, '$.upload_uptime_ms') >= json_extract(payload, '$.uptime_ms')
                    AND json_extract(payload, '$.upload_uptime_ms') - json_extract(payload, '$.uptime_ms') <= 300000
                    THEN 'estimated capture' ELSE 'backend receipt' END AS time_basis,
                json_extract(payload, '$.channel') AS channel,
                json_extract(payload, '$.observed_ms') AS observed_ms,
                json_extract(payload, '$.capture_gap_ms') AS gap_ms,
                json_extract(payload, '$.frames') AS frames,
                json_extract(payload, '$.management_frames') AS management_frames,
                json_extract(payload, '$.data_frames') AS data_frames,
                json_extract(payload, '$.data_retries') AS data_retries,
                json_extract(payload, '$.networks') AS networks,
                json_extract(payload, '$.active_radios') AS active_radios,
                json_extract(payload, '$.rssi_avg_dbm') AS rssi_dbm,
                json_extract(payload, '$.queue_dropped') AS queue_dropped,
                json_extract(payload, '$.malformed') AS malformed,
                json_extract(payload, '$.prefix_truncated') AS prefix_truncated,
                json_extract(payload, '$.rx_errors') AS rx_errors,
                json_extract(payload, '$.evictions') AS evictions
            FROM messages WHERE {scope}
                AND received_ms >= ${{__from}} AND received_ms < ${{__to}} + 300000
        )"#
    )
}

fn place(value: &mut Value, x: usize, y: usize, w: usize, h: usize) {
    value["gridPos"] = json!({"x": x, "y": y, "w": w, "h": h});
}

fn stat(id: usize, title: &str, description: &str, sql: String, unit: &str, x: usize) -> Value {
    let mut value = panel(id, title, description, "stat", sql, unit);
    place(&mut value, x, 0, 8, 4);
    value["options"] = json!({
        "colorMode": "value", "graphMode": "none", "textMode": "value_and_name",
        "reduceOptions": {"calcs": ["lastNotNull"], "fields": "", "values": false}
    });
    value
}

fn points(
    id: usize,
    title: &str,
    description: &str,
    sql: String,
    unit: &str,
    x: usize,
    y: usize,
) -> Value {
    let mut value = panel(id, title, description, "timeseries", sql, unit);
    place(&mut value, x, y, 12, 8);
    value["fieldConfig"]["defaults"]["custom"] = json!({
        "drawStyle": "points", "showPoints": "always", "pointSize": 5,
        "lineWidth": 0, "spanNulls": false, "fillOpacity": 0
    });
    value
}

fn bars(
    id: usize,
    title: &str,
    description: &str,
    sql: String,
    unit: &str,
    x: usize,
    y: usize,
    stacked: bool,
) -> Value {
    let mut value = panel(id, title, description, "timeseries", sql, unit);
    place(&mut value, x, y, 12, 8);
    value["fieldConfig"]["defaults"]["custom"] = json!({
        "drawStyle": "bars", "fillOpacity": 75, "lineWidth": 0,
        "stacking": {"mode": if stacked { "normal" } else { "none" }, "group": "A"},
        "spanNulls": false
    });
    value
}

pub(super) fn dashboard(contract: &Contract) -> Value {
    let scope = format!(
        "contract = '{}' AND device IN (${{device:sqlstring}})",
        contract.id
    );
    let source = scan_cte(&scope);
    let mut panels = Vec::new();

    panels.push(stat(
        1,
        "Last report age",
        "Seconds since the backend accepted a scanner report. A survey and upload normally take over a minute; this is reporting freshness, not radio traffic.",
        format!("SELECT MAX(received_ms) / 1000.0 AS time, ROUND((unixepoch('now') * 1000 - MAX(received_ms)) / 1000.0, 0) AS age_s FROM messages WHERE {scope}"),
        "s", 0,
    ));
    panels.push(stat(
        2,
        "Latest blind interval",
        "Capture-stopped time immediately before the latest window. It includes reporting and optional MQTT upload, but not time spent surveying other channels.",
        format!("SELECT received_ms / 1000.0 AS time, json_extract(payload, '$.capture_gap_ms') AS gap_ms FROM messages WHERE {scope} ORDER BY received_ms DESC LIMIT 1"),
        "ms", 8,
    ));
    panels.push(stat(
        3,
        "Queue drops in range",
        "Sum of callback frames lost in the processing queue across accepted windows in the selected time range. Zero does not prove complete radio capture.",
        format!("{source} SELECT MAX(capture_ms) / 1000.0 AS time, SUM(queue_dropped) AS drops FROM scan WHERE {CAPTURE_RANGE}"),
        "short", 16,
    ));

    let mut channels = panel(
        4,
        "Latest window on each receive channel",
        "One latest report per channel, not necessarily one complete sweep. Frames/s uses actual observed duration. Retry share is shown only with at least 20 data frames; it is not packet loss. RSSI is measured at the scanner. Capture time is estimated from publish uptime when available, otherwise backend receipt time.",
        "table",
        format!(
            r#"{source}, ranked AS (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY device, channel ORDER BY capture_ms DESC, id DESC) AS rn
                FROM scan WHERE {CAPTURE_RANGE}
            )
            SELECT channel,
                ROUND(frames * 1000.0 / MAX(1, observed_ms), 1) AS frames_per_s,
                networks, active_radios, rssi_dbm,
                CASE WHEN data_frames >= 20 THEN ROUND(data_retries * 100.0 / data_frames, 1) END AS retry_percent,
                ROUND((unixepoch('now') * 1000 - received_ms) / 1000.0, 0) AS report_age_s,
                time_basis
            FROM ranked WHERE rn = 1 ORDER BY device, channel"#
        ),
        "none",
    );
    place(&mut channels, 0, 4, 24, 11);
    channels["targets"][0]["timeColumns"] = json!([]);
    channels["fieldConfig"]["overrides"] = json!([
        {"matcher":{"id":"byName","options":"frames_per_s"},"properties":[
            {"id":"displayName","value":"Frames/s"},
            {"id":"custom.cellOptions","value":{"type":"gauge","mode":"basic"}},
            {"id":"min","value":0}]},
        {"matcher":{"id":"byName","options":"rssi_dbm"},"properties":[
            {"id":"displayName","value":"RSSI at scanner"},{"id":"unit","value":"dBm"}]},
        {"matcher":{"id":"byName","options":"retry_percent"},"properties":[
            {"id":"displayName","value":"Retry %"},{"id":"unit","value":"percent"},
            {"id":"custom.cellOptions","value":{"type":"color-background","mode":"basic"}},
            {"id":"thresholds","value":{"mode":"absolute","steps":[
                {"color":"green","value":null},{"color":"orange","value":20},{"color":"red","value":40}]}}]},
        {"matcher":{"id":"byName","options":"report_age_s"},"properties":[
            {"id":"displayName","value":"Report age"},{"id":"unit","value":"s"}]}
    ]);
    panels.push(channels);

    let selected = format!("{CAPTURE_RANGE} AND {CHANNEL_FILTER}");
    let grouped = format!("GROUP BY time, device, channel ORDER BY time");
    panels.push(points(
        5, "Captured frames/s", "Delivered frames divided by observed capture time, grouped by capture-time bucket and receive channel. Points mark observed windows; no line is drawn across unobserved time.",
        format!("{source} SELECT {BUCKET} AS time, device || ' ch ' || channel AS device, SUM(frames) * 1000.0 / MAX(1, SUM(observed_ms)) AS value FROM scan WHERE {selected} {grouped}"),
        "short", 0, 15,
    ));
    panels.push(points(
        6, "Average RSSI at scanner", "Mean signal of captured frames at the scanner antenna, weighted by frame count when windows share a bucket. This is not the IoT device's own RSSI.",
        format!("{source} SELECT {BUCKET} AS time, device || ' ch ' || channel AS device, SUM(rssi_dbm * frames) * 1.0 / NULLIF(SUM(CASE WHEN rssi_dbm IS NOT NULL THEN frames ELSE 0 END), 0) AS value FROM scan WHERE {selected} AND rssi_dbm IS NOT NULL GROUP BY time, device, channel ORDER BY time"),
        "dBm", 12, 15,
    ));
    panels.push(points(
        7, "Observed data retry share", "Captured data frames with the retry bit set, weighted by data-frame count. Buckets with fewer than 20 data frames are hidden; this is not packet loss.",
        format!("{source} SELECT {BUCKET} AS time, device || ' ch ' || channel AS device, CASE WHEN SUM(data_frames) >= 20 THEN SUM(data_retries) * 100.0 / SUM(data_frames) END AS value FROM scan WHERE {selected} GROUP BY time, device, channel ORDER BY time"),
        "percent", 0, 23,
    ));
    panels.push(bars(
        8, "Captured traffic mix", "Management and data frames per observed second on the selected receive channel. Other delivered frames and parse/RX errors are intentionally not included in this stack.",
        format!(r#"{source}, buckets AS (
            SELECT {BUCKET} AS time, device, channel,
                SUM(management_frames) * 1000.0 / MAX(1, SUM(observed_ms)) AS management_per_s,
                SUM(data_frames) * 1000.0 / MAX(1, SUM(observed_ms)) AS data_per_s
            FROM scan WHERE {selected} GROUP BY time, device, channel
        )
        SELECT time, device || ' ch ' || channel || ' management' AS device, management_per_s AS value FROM buckets
        UNION ALL SELECT time, device || ' ch ' || channel || ' data', data_per_s FROM buckets ORDER BY time"#),
        "short", 12, 23, true,
    ));
    panels.push(bars(
        9, "Blind interval before window", "Time capture was stopped immediately before each selected-channel window. Survey time on other channels is not counted as a blind interval here.",
        format!("{source} SELECT capture_ms / 1000.0 AS time, device || ' ch ' || channel AS device, gap_ms AS value FROM scan WHERE {selected} ORDER BY time"),
        "ms", 0, 31, false,
    ));
    panels.push(bars(
        10, "Capture processing issues", "Counts of callback queue drops, malformed headers, truncated prefixes, delivered RX errors and table evictions per selected-channel window. Earlier radio loss is unknown.",
        format!(r#"{source}, issues AS (
            SELECT capture_ms / 1000.0 AS time, device, channel, queue_dropped, malformed, prefix_truncated, rx_errors, evictions
            FROM scan WHERE {selected}
        )
        SELECT time, device || ' ch ' || channel || ' queue drops' AS device, queue_dropped AS value FROM issues
        UNION ALL SELECT time, device || ' ch ' || channel || ' malformed', malformed FROM issues
        UNION ALL SELECT time, device || ' ch ' || channel || ' truncated', prefix_truncated FROM issues
        UNION ALL SELECT time, device || ' ch ' || channel || ' RX errors', rx_errors FROM issues
        UNION ALL SELECT time, device || ' ch ' || channel || ' evictions', evictions FROM issues
        ORDER BY time"#),
        "short", 12, 31, true,
    ));

    let mut windows = panel(
        12,
        "Recent scan windows",
        "Individual accepted reports, newest first. Capture time is estimated when upload uptime is available; older records show backend receipt time.",
        "table",
        format!(
            "{source} SELECT capture_ms / 1000.0 AS time, received_ms / 1000.0 AS received_time, device, channel, observed_ms, frames, data_frames, data_retries, rssi_dbm, gap_ms, queue_dropped, time_basis FROM scan WHERE {CAPTURE_RANGE} ORDER BY capture_ms DESC LIMIT 100"
        ),
        "none",
    );
    place(&mut windows, 0, 40, 24, 8);
    let mut payloads = panel(
        13,
        "Raw accepted payloads",
        "Raw accepted JSON messages in the selected receipt-time range, newest first.",
        "table",
        format!(
            "SELECT received_ms / 1000.0 AS time, device, payload FROM messages WHERE {scope} AND received_ms >= ${{__from}} AND received_ms < ${{__to}} ORDER BY received_ms DESC LIMIT 100"
        ),
        "none",
    );
    place(&mut payloads, 0, 48, 12, 8);
    let mut rejected = panel(
        14, "Rejected messages (all contracts)", "Backend-wide contract-validation failures; this ignores the device selector.",
        "table",
        "SELECT received_ms / 1000.0 AS time, topic, reason FROM rejected WHERE received_ms >= ${__from} AND received_ms < ${__to} ORDER BY received_ms DESC LIMIT 100".into(),
        "none",
    );
    place(&mut rejected, 12, 48, 12, 8);
    panels.push(json!({
        "id": 11, "type": "row", "title": "Raw diagnostics", "collapsed": true,
        "description": "Individual windows, accepted JSON payloads and backend-wide rejected messages for deeper inspection.",
        "gridPos": {"x": 0, "y": 39, "w": 24, "h": 1},
        "panels": [windows, payloads, rejected]
    }));

    json!({
        "uid": format!("sdk-{}", contract.id), "title": contract.title,
        "description": "Passive 2.4 GHz scan windows. Compare the same receive channel. Capture times are estimates anchored to MQTT publication; older reports use backend receipt time. Counts describe observations at the scanner, not packet loss or device health.",
        "schemaVersion": 39, "version": 2, "editable": false,
        "tags": ["embedded-sdk", contract.id], "timezone": "browser",
        "refresh": "5s", "time": {"from": "now-1h", "to": "now"},
        "templating": {"list": [
            {"name": "device", "label": "Scanner", "type": "query", "datasource": datasource(),
                "query": format!("SELECT DISTINCT device FROM messages WHERE contract = '{}' ORDER BY device", contract.id),
                "refresh": 2, "multi": false, "includeAll": false, "current": {}, "options": []},
            {"name": "channel", "label": "Detail channel", "type": "query", "datasource": datasource(),
                "query": format!("SELECT CAST(json_extract(payload, '$.channel') AS TEXT) AS __text, CAST(json_extract(payload, '$.channel') AS TEXT) AS __value FROM messages WHERE contract = '{}' AND device IN (${{device:sqlstring}}) GROUP BY __value ORDER BY MAX(received_ms) DESC", contract.id),
                "refresh": 2, "multi": false, "includeAll": false, "current": {}, "options": []}
        ]},
        "panels": panels
    })
}
