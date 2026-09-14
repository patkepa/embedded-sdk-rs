use std::{path::Path, time::Duration};

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::Contract;

/// Single writer holding the embedded database. Grafana opens a read-only connection.
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Open or initialize the database with time indexes and crash-safe transactions.
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(include_str!("schema.sql"))?;
        Ok(Self { connection })
    }

    /// Store a complete message atomically, or quarantine it with a validation reason.
    /// Returns false for a quarantined message; database errors propagate to the caller.
    pub fn ingest(
        &mut self,
        contracts: &[Contract],
        topic: &str,
        payload: &[u8],
        received_ms: i64,
    ) -> Result<bool> {
        let parsed = (|| -> Result<_> {
            let (contract, device) = contracts
                .iter()
                .find_map(|c| c.device(topic).map(|d| (c, d)))
                .ok_or_else(|| anyhow::anyhow!("unknown topic"))?;
            anyhow::ensure!(payload.len() <= 65_536, "payload exceeds 64 KiB");
            let json: serde_json::Value =
                serde_json::from_slice(payload).map_err(|_| anyhow::anyhow!("invalid JSON"))?;
            let values = contract.extract(&json)?;
            Ok((contract, device, values))
        })();
        let (contract, device, values) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                self.connection.execute(
                    "INSERT INTO rejected(received_ms, topic, payload, reason) VALUES (?1, ?2, ?3, ?4)",
                    params![received_ms, topic, &payload[..payload.len().min(65_536)], error.to_string()],
                )?;
                return Ok(false);
            }
        };
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO messages(received_ms, contract, device, topic, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![received_ms, contract.id, device, topic, std::str::from_utf8(payload)?],
        )?;
        let message = transaction.last_insert_rowid();
        for (metric, value) in values {
            transaction.execute(
                "INSERT INTO samples(message_id, received_ms, contract, device, metric, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![message, received_ms, contract.id, device, metric, value],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }

    /// Delete old messages, their samples, and rejected messages. Freed pages are reused.
    pub fn prune(&mut self, before_ms: i64) -> Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM messages WHERE received_ms < ?1", [before_ms])?;
        transaction.execute("DELETE FROM rejected WHERE received_ms < ?1", [before_ms])?;
        transaction.commit()?;
        Ok(())
    }
}
