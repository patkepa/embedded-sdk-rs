PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    received_ms INTEGER NOT NULL,
    contract TEXT NOT NULL,
    device TEXT NOT NULL,
    topic TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_time ON messages(received_ms);
CREATE INDEX IF NOT EXISTS messages_contract_device_time ON messages(contract, device, received_ms);
CREATE TABLE IF NOT EXISTS samples (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    received_ms INTEGER NOT NULL,
    contract TEXT NOT NULL,
    device TEXT NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY(message_id, metric)
);
CREATE INDEX IF NOT EXISTS samples_series_time ON samples(contract, metric, device, received_ms);
CREATE TABLE IF NOT EXISTS rejected (
    id INTEGER PRIMARY KEY,
    received_ms INTEGER NOT NULL,
    topic TEXT NOT NULL,
    payload BLOB NOT NULL,
    reason TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS rejected_time ON rejected(received_ms);
