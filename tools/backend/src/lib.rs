//! File-based telemetry contracts, transactional SQLite ingestion, and Grafana dashboards.

mod contract;
mod dashboard;
mod storage;

pub use contract::{Contract, Metric, load_contracts, validate_contracts};
pub use dashboard::dashboard;
pub use storage::Store;
