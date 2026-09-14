use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One versioned wire format, owned by a firmware package.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    /// Stable identifier; change this when making incompatible schema changes.
    pub id: String,
    /// Grafana dashboard title.
    pub title: String,
    /// Required integer in the payload's `version` field.
    pub version: u64,
    /// MQTT topic with exactly one whole-level `{device}` placeholder.
    pub topic: String,
    /// Required constant values, addressed by JSON Pointer.
    #[serde(default)]
    pub equals: BTreeMap<String, Value>,
    /// Numeric fields to store and graph.
    pub metrics: Vec<Metric>,
    /// Valid example, also usable to smoke-test the pipeline without hardware.
    pub example: Value,
}

/// One numeric time series extracted from each payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    /// Stable field identifier within a contract.
    pub key: String,
    /// Panel title.
    pub title: String,
    /// JSON Pointer to a number in the payload, e.g. `/temperature_c`.
    pub pointer: String,
    /// Grafana unit identifier, e.g. `celsius`, `dBm`, or `none`.
    pub unit: String,
    /// Whether a missing field rejects the entire message (defaults to true).
    #[serde(default = "required")]
    pub required: bool,
}

fn required() -> bool {
    true
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

fn pointer(value: &str) -> bool {
    value.starts_with('/')
        && !value.as_bytes().contains(&0)
        && value
            .split('~')
            .skip(1)
            .all(|part| part.starts_with('0') || part.starts_with('1'))
}

impl Contract {
    /// Validate the schema and its example before starting any services.
    pub fn validate(&self) -> Result<()> {
        ensure!(identifier(&self.id), "invalid contract id");
        ensure!(
            !self.title.is_empty() && self.version > 0,
            "empty title or zero version"
        );
        let levels: Vec<_> = self.topic.split('/').collect();
        ensure!(
            levels.iter().filter(|&&level| level == "{device}").count() == 1,
            "topic must contain exactly one {{device}} level"
        );
        ensure!(
            self.topic.len() <= 256
                && !self.topic.starts_with('$')
                && levels.iter().all(|level| !level.is_empty()
                    && (*level == "{device}" || !level.contains(['+', '#', '{', '}', '\0']))),
            "invalid topic template"
        );
        let mut keys = std::collections::BTreeSet::new();
        for metric in &self.metrics {
            ensure!(
                identifier(&metric.key) && keys.insert(&metric.key),
                "invalid or duplicate metric key"
            );
            ensure!(pointer(&metric.pointer), "invalid metric JSON Pointer");
            ensure!(
                !metric.title.is_empty() && !metric.unit.is_empty(),
                "empty metric title or unit"
            );
        }
        ensure!(
            self.equals.keys().all(|key| pointer(key)),
            "invalid constant JSON Pointer"
        );
        self.extract(&self.example)
            .context("contract example does not satisfy its schema")?;
        Ok(())
    }

    /// MQTT wildcard subscription corresponding to this contract.
    pub fn filter(&self) -> String {
        self.topic.replace("{device}", "+")
    }

    /// Return the device identity only when the complete topic matches.
    pub fn device<'a>(&self, topic: &'a str) -> Option<&'a str> {
        let expected: Vec<_> = self.topic.split('/').collect();
        let actual: Vec<_> = topic.split('/').collect();
        if expected.len() != actual.len() {
            return None;
        }
        let mut device = None;
        for (expected, actual) in expected.into_iter().zip(actual) {
            if expected == "{device}" {
                if actual.is_empty() || actual.contains(['+', '#', '\0']) {
                    return None;
                }
                device = Some(actual);
            } else if expected != actual {
                return None;
            }
        }
        device
    }

    pub(crate) fn extract(&self, payload: &Value) -> Result<Vec<(&str, f64)>> {
        ensure!(payload.is_object(), "payload must be a JSON object");
        ensure!(
            payload.get("version").and_then(Value::as_u64) == Some(self.version),
            "unsupported payload version"
        );
        for (pointer, expected) in &self.equals {
            ensure!(
                payload.pointer(pointer) == Some(expected),
                "constant mismatch at {pointer}"
            );
        }
        let mut values = Vec::new();
        for metric in &self.metrics {
            match payload.pointer(&metric.pointer) {
                None if !metric.required => continue,
                Some(value) => match value.as_f64() {
                    Some(value) if value.is_finite() => values.push((metric.key.as_str(), value)),
                    _ => bail!("non-numeric field {}", metric.pointer),
                },
                None => bail!("missing field {}", metric.pointer),
            }
        }
        Ok(values)
    }
}

/// Reject ambiguous routes or duplicate IDs so ingestion cannot pick the wrong schema.
pub fn validate_contracts(contracts: &[Contract]) -> Result<()> {
    ensure!(!contracts.is_empty(), "no telemetry contracts registered");
    for (index, contract) in contracts.iter().enumerate() {
        contract
            .validate()
            .with_context(|| format!("contract {}", contract.id))?;
        for other in &contracts[..index] {
            ensure!(
                contract.id != other.id,
                "duplicate contract id {}",
                contract.id
            );
            let left: Vec<_> = contract.topic.split('/').collect();
            let right: Vec<_> = other.topic.split('/').collect();
            let overlaps = left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(a, b)| *a == b || *a == "{device}" || b == "{device}");
            ensure!(
                !overlaps,
                "overlapping topics for {} and {}",
                contract.id,
                other.id
            );
        }
    }
    Ok(())
}

/// Read a directory of JSON contract files, sorted for reproducible provisioning.
pub fn load_contracts(directory: &Path) -> Result<Vec<Contract>> {
    let mut files = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    files.sort();
    let contracts = files
        .iter()
        .map(|path| {
            serde_json::from_slice(&fs::read(path)?)
                .with_context(|| format!("reading {}", path.display()))
        })
        .collect::<Result<Vec<Contract>>>()?;
    validate_contracts(&contracts)?;
    Ok(contracts)
}
