//! Repository automation for host checks and ESP32-C6 firmware workflows.

use std::{
    env,
    ffi::OsStr,
    process::{Command, ExitCode, Stdio},
};

const ESP32C6_TARGET: &str = "riscv32imac-unknown-none-elf";
const XIAO_ESP32C6_BEACON: &str = "xiao-esp32c6-beacon";
const XIAO_ESP32C6_BEACON_SCANNER: &str = "xiao-esp32c6-beacon-scanner";
const XIAO_ESP32C6_FIRMWARE: &str = "xiao-esp32c6-firmware";

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "help".to_owned());

    let result = match command.as_str() {
        "build-xiao-esp32c6" => cargo([
            "build",
            "-p",
            XIAO_ESP32C6_FIRMWARE,
            "--target",
            ESP32C6_TARGET,
            "--release",
        ]),
        "build-xiao-esp32c6-beacon" => cargo([
            "build",
            "-p",
            XIAO_ESP32C6_BEACON,
            "--target",
            ESP32C6_TARGET,
            "--release",
        ]),
        "build-xiao-esp32c6-beacon-scanner" => cargo([
            "build",
            "-p",
            XIAO_ESP32C6_BEACON_SCANNER,
            "--target",
            ESP32C6_TARGET,
            "--release",
        ]),
        "check" => check(),
        "doctor" => doctor(),
        "run-xiao-esp32c6" => cargo([
            "run",
            "-p",
            XIAO_ESP32C6_FIRMWARE,
            "--target",
            ESP32C6_TARGET,
            "--release",
        ]),
        "run-xiao-esp32c6-beacon" => cargo([
            "run",
            "-p",
            XIAO_ESP32C6_BEACON,
            "--target",
            ESP32C6_TARGET,
            "--release",
        ]),
        "run-xiao-esp32c6-beacon-scanner" => cargo([
            "run",
            "-p",
            XIAO_ESP32C6_BEACON_SCANNER,
            "--target",
            ESP32C6_TARGET,
            "--release",
        ]),
        "test" => host_tests(),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        unknown => Err(format!("unknown xtask command: {unknown}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn check() -> Result<(), String> {
    cargo(["fmt", "--all", "--check"])?;
    cargo([
        "clippy",
        "--workspace",
        "--exclude",
        XIAO_ESP32C6_FIRMWARE,
        "--exclude",
        XIAO_ESP32C6_BEACON,
        "--exclude",
        XIAO_ESP32C6_BEACON_SCANNER,
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    host_tests()
}

fn host_tests() -> Result<(), String> {
    cargo([
        "test",
        "--workspace",
        "--exclude",
        XIAO_ESP32C6_FIRMWARE,
        "--exclude",
        XIAO_ESP32C6_BEACON,
        "--exclude",
        XIAO_ESP32C6_BEACON_SCANNER,
    ])
}

fn doctor() -> Result<(), String> {
    let installed = output("rustup", ["target", "list", "--installed"])?;
    if !installed.lines().any(|target| target == ESP32C6_TARGET) {
        return Err(format!(
            "missing Rust target {ESP32C6_TARGET}; run `rustup target add {ESP32C6_TARGET}`"
        ));
    }

    if command("espflash", ["--version"]).is_err() {
        return Err("espflash is unavailable; install it with `cargo install espflash`".to_owned());
    }

    println!("Rust target: {ESP32C6_TARGET}");
    println!("Flasher: espflash");
    println!("ESP32-C6 development environment is ready");
    Ok(())
}

fn cargo<const N: usize>(args: [&str; N]) -> Result<(), String> {
    let executable = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    command(executable, args)
}

fn command<I, S>(program: impl AsRef<OsStr>, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to start {}: {error}", program.to_string_lossy()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} exited with status {status}",
            program.to_string_lossy()
        ))
    }
}

fn output<I, S>(program: impl AsRef<OsStr>, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("failed to start {}: {error}", program.to_string_lossy()))?;

    if !output.status.success() {
        return Err(format!(
            "{} exited with status {}",
            program.to_string_lossy(),
            output.status
        ));
    }

    String::from_utf8(output.stdout).map_err(|error| format!("command returned non-UTF-8: {error}"))
}

fn print_help() {
    println!("embedded-sdk repository tasks");
    println!();
    println!("  cargo xtask check          format, lint, and test host packages");
    println!("  cargo xtask test           run host tests");
    println!("  cargo xtask doctor         verify ESP32-C6 development tools");
    println!("  cargo xtask build-xiao-esp32c6  build release firmware");
    println!("  cargo xtask run-xiao-esp32c6    build, flash, and monitor firmware");
    println!("  cargo xtask build-xiao-esp32c6-beacon  build iBeacon firmware");
    println!("  cargo xtask run-xiao-esp32c6-beacon    build, flash, and monitor iBeacon");
    println!("  cargo xtask build-xiao-esp32c6-beacon-scanner  build BLE scanner firmware");
    println!(
        "  cargo xtask run-xiao-esp32c6-beacon-scanner    build, flash, and monitor BLE scanner"
    );
}
