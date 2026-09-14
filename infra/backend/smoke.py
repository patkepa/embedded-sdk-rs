#!/usr/bin/env python3
"""Exercise the running MQTT -> SQLite -> Grafana pipeline using only Python stdlib."""

import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
BASE = f"http://127.0.0.1:{os.environ.get('GRAFANA_PORT', '3000')}"
SOURCE = {"type": "frser-sqlite-datasource", "uid": "embedded-sdk-sqlite"}
DEVICE = f"smoke-{time.time_ns()}"


def request(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as response:
        return json.load(response)


def query(sql, query_type="table", time_columns=None):
    result = request("/api/ds/query", {
        "from": str(int((time.time() - 3600) * 1000)),
        "to": str(int((time.time() + 60) * 1000)),
        "queries": [{"refId": "A", "datasource": SOURCE, "queryText": sql,
                     "rawQueryText": sql, "queryType": query_type,
                     "timeColumns": time_columns or []}],
    })["results"]["A"]
    assert not result.get("error"), result
    assert result.get("status", 200) == 200, result
    return result.get("frames", [])


def count(sql):
    return query(sql)[0]["data"]["values"][0][0]


def wait_for_count(sql, expected):
    deadline = time.monotonic() + 15
    while count(sql) != expected:
        if time.monotonic() >= deadline:
            raise AssertionError(f"Expected {expected}: {sql}")
        time.sleep(0.2)


def publish(topic, payload):
    subprocess.run([
        "docker", "compose", "--project-name", "embedded-sdk-dev",
        "--file", str(ROOT / "infra/backend/compose.yaml"),
        "exec", "-T", "mosquitto", "mosquitto_pub", "-h", "localhost",
        "-V", "mqttv5", "-q", "1", "-t", topic, "-m", payload,
    ], cwd=ROOT, check=True)


def main():
    assert request("/api/datasources/uid/embedded-sdk-sqlite/health")["status"] == "OK"
    contracts = json.loads((ROOT / "target/dev-backend/contracts-index.json").read_text())
    for contract in contracts:
        topic = contract["topic"].replace("{device}", DEVICE)
        publish(topic, json.dumps(contract["example"]))
        scope = f"contract = '{contract['id']}' AND device = '{DEVICE}'"
        wait_for_count(f"SELECT COUNT(*) FROM messages WHERE {scope}", 1)
        publish(topic, "not-json")
        escaped_topic = topic.replace("'", "''")
        wait_for_count(f"SELECT COUNT(*) FROM rejected WHERE topic = '{escaped_topic}'", 1)
        assert count(f"SELECT COUNT(*) FROM messages WHERE {scope}") == 1
        provisioned = request(f"/api/dashboards/uid/sdk-{contract['id']}")
        assert provisioned["meta"]["provisioned"]
        dashboard = provisioned["dashboard"]
        # The variable query must discover devices through the real plugin too.
        variables = query(dashboard["templating"]["list"][0]["query"])
        assert DEVICE in variables[0]["data"]["values"][0]
        for panel in dashboard["panels"]:
            target = panel["targets"][0]
            sql = (target["rawQueryText"].replace("${device:sqlstring}", f"'{DEVICE}'")
                   .replace("${__from}", str(int((time.time() - 3600) * 1000)))
                   .replace("${__to}", str(int((time.time() + 60) * 1000)))
                   .replace("${__interval_ms}", "1000"))
            frames = query(sql, target["queryType"], target["timeColumns"])
            if panel["type"] == "timeseries":
                assert any(field["type"] == "time" for frame in frames
                           for field in frame["schema"]["fields"]), panel["title"]
        print(f"PASS {contract['id']}: MQTT 5, persistence, quarantine, device discovery, Grafana panels")
    print(f"Smoke device: {DEVICE} (test data expires with normal retention)")


if __name__ == "__main__":
    main()
