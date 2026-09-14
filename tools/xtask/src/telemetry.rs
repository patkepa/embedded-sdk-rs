use std::{
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

use cargo_metadata::MetadataCommand;
use clap::Subcommand;
use embedded_sdk_dev_backend::{Contract, dashboard, validate_contracts};
use serde_json::json;

#[derive(Subcommand)]
pub(super) enum Action {
    /// Validate contracts, build, and start the full stack in the background (default).
    Up,
    /// Stop the stack, preserving all database volumes.
    Down,
    /// Follow backend logs (Ctrl-C leaves the stack running).
    Logs,
    /// Print container status.
    Status,
    /// Validate contracts and generate provisioning without Docker.
    Prepare,
    /// Publish each contract's example as device `demo` (starts the stack first).
    Demo,
}

pub(super) fn run(action: Option<Action>) -> Result<(), String> {
    execute(action.unwrap_or(Action::Up)).map_err(|error| error.to_string())
}

fn execute(action: Action) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = MetadataCommand::new().no_deps().exec()?;
    let root = metadata.workspace_root.as_std_path();
    let generated = root.join("target/dev-backend");
    if matches!(action, Action::Down | Action::Logs | Action::Status) {
        let args: &[&str] = match action {
            Action::Down => &["down"],
            Action::Logs => &["logs", "--follow", "--tail", "100"],
            _ => &["ps"],
        };
        compose(root, args)?;
        return Ok(());
    }
    let mut contracts = Vec::new();
    for package in &metadata.packages {
        let Some(path) = package
            .metadata
            .get("embedded-sdk")
            .and_then(|v| v.get("firmware"))
            .and_then(|v| v.get("telemetry-contract"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let path = package.manifest_path.parent().unwrap().join(path);
        let contract: Contract = serde_json::from_slice(&fs::read(&path)?)
            .map_err(|error| format!("{}: {error}", path))?;
        contracts.push(contract);
    }
    contracts.sort_by(|left, right| left.id.cmp(&right.id));
    validate_contracts(&contracts).map_err(|error| format!("{error:#}"))?;
    // Remove only previously generated JSON files, including contracts removed from metadata.
    for directory in ["contracts", "dashboards"] {
        let path = generated.join(directory);
        fs::create_dir_all(&path)?;
        for entry in fs::read_dir(&path)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                fs::remove_file(path)?;
            }
        }
    }
    for contract in &contracts {
        write_json(
            generated.join(format!("contracts/{}.json", contract.id)),
            contract,
        )?;
        write_json(
            generated.join(format!("dashboards/{}.json", contract.id)),
            &dashboard(contract),
        )?;
        println!("{}: {}", contract.id, contract.filter());
    }
    write_json(generated.join("contracts-index.json"), &json!(contracts))?;
    if matches!(action, Action::Prepare) {
        println!("Provisioning ready in {}", generated.display());
        return Ok(());
    }
    compose(
        root,
        &[
            "up",
            "--build",
            "--detach",
            "--wait",
            "--wait-timeout",
            "120",
        ],
    )?;
    println!(
        "Grafana: http://localhost:{} (anonymous viewer; admin / admin for local editing)",
        std::env::var("GRAFANA_PORT").unwrap_or_else(|_| "3000".into())
    );
    println!(
        "MQTT: this computer's LAN address:{}; point firmware MQTT_HOST here before flashing",
        std::env::var("BACKEND_MQTT_PORT").unwrap_or_else(|_| "1883".into())
    );
    println!("Stop: cargo xtask telemetry down (keeps data)");
    if matches!(action, Action::Demo) {
        for contract in &contracts {
            compose(
                root,
                &[
                    "exec",
                    "-T",
                    "mosquitto",
                    "mosquitto_pub",
                    "-h",
                    "localhost",
                    "-t",
                    &contract.topic.replace("{device}", "demo"),
                    "-q",
                    "1",
                    "-m",
                    &serde_json::to_string(&contract.example)?,
                ],
            )?;
        }
        println!("Published examples for device demo");
    }
    Ok(())
}

fn write_json(
    path: PathBuf,
    value: &impl serde::Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn compose(root: &Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    // Changing a contract recreates the collector and starts a matching MQTT session.
    let mut revision = std::collections::hash_map::DefaultHasher::new();
    fs::read(root.join("target/dev-backend/contracts-index.json"))
        .unwrap_or_default()
        .hash(&mut revision);
    let status = Command::new("docker")
        .current_dir(root)
        .env(
            "BACKEND_CONTRACT_REVISION",
            format!("{:016x}", revision.finish()),
        )
        .args([
            "compose",
            "--project-name",
            "embedded-sdk-dev",
            "--file",
            "infra/backend/compose.yaml",
        ])
        .args(args)
        .status()?;
    if !status.success() {
        return Err(format!("docker compose exited with {status}").into());
    }
    Ok(())
}
