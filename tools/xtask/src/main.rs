//! Repository automation for host checks and ESP32-C6 firmware workflows.

use std::{
    env,
    ffi::OsStr,
    io::ErrorKind as IoErrorKind,
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

use embedded_sdk_provisioning::{
    DeviceState, ErrorKind, MAX_REQUEST_BYTES, MAX_TRANSPORT_FRAME_BYTES, Request, RequestId,
    ResponseKind, TransactionId, TransactionState, WireResponse, decode_response, encode_request,
};
use serialport::SerialPort;
use xiao_esp32c6_config::{
    CURRENT_SCHEMA, SerialFrameDecoder, SerialFrameKind, encode_serial_frame,
};
use zeroize::Zeroize;

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
        "build-xiao-esp32c6-hil" => cargo([
            "build",
            "-p",
            XIAO_ESP32C6_FIRMWARE,
            "--target",
            ESP32C6_TARGET,
            "--release",
            "--no-default-features",
            "--features",
            "hil-provisioning",
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
        "hil-smoke-xiao-esp32c6" => env::args()
            .nth(2)
            .ok_or_else(|| "usage: cargo xtask hil-smoke-xiao-esp32c6 <serial-port>".to_owned())
            .and_then(|port| hil_smoke_xiao_esp32c6(&port)),
        "hil-reset-xiao-esp32c6" => env::args()
            .nth(2)
            .ok_or_else(|| "usage: cargo xtask hil-reset-xiao-esp32c6 <serial-port>".to_owned())
            .and_then(|port| hil_reset_xiao_esp32c6(&port)),
        "hil-negative-xiao-esp32c6" => env::args()
            .nth(2)
            .ok_or_else(|| "usage: cargo xtask hil-negative-xiao-esp32c6 <serial-port>".to_owned())
            .and_then(|port| hil_negative_xiao_esp32c6(&port)),
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

fn hil_smoke_xiao_esp32c6(port_path: &str) -> Result<(), String> {
    const OPEN_TEST_CONFIGURATION: &[u8] = b"XCF1\0\0\x04\0\0wifi";

    command(
        "espflash",
        ["reset", "--chip", "esp32c6", "--port", port_path],
    )?;
    let mut port = open_serial_after_reset(port_path)?;
    let mut decoder = SerialFrameDecoder::new();
    let transaction_id = TransactionId::new(1).expect("constant transaction ID is nonzero");

    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Begin {
                request_id: request_id(1),
                transaction_id,
                schema: CURRENT_SCHEMA,
            },
        )?,
        1,
        |kind| matches!(kind, ResponseKind::TransactionBegun),
        "begin",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::SubmitCandidate {
                request_id: request_id(2),
                transaction_id,
                encoded: OPEN_TEST_CONFIGURATION,
            },
        )?,
        2,
        |kind| matches!(kind, ResponseKind::CandidateReceived),
        "submit",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Validate {
                request_id: request_id(3),
                transaction_id,
            },
        )?,
        3,
        |kind| matches!(kind, ResponseKind::CandidateValidated),
        "validate",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Commit {
                request_id: request_id(4),
                transaction_id,
            },
        )?,
        4,
        |kind| matches!(kind, ResponseKind::Committed(_)),
        "commit",
    )?;

    println!("HIL provisioning smoke candidate committed; device reboot expected");
    println!("The built-in open-network candidate is intentionally expected to fail verification");

    drop(port);
    thread::sleep(Duration::from_secs(1));
    command(
        "espflash",
        ["reset", "--chip", "esp32c6", "--port", port_path],
    )?;
    let mut port = open_serial_after_reset(port_path)?;
    let mut decoder = SerialFrameDecoder::new();
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Status {
                request_id: request_id(5),
            },
        )?,
        5,
        |kind| {
            matches!(
                kind,
                ResponseKind::Status(status)
                    if status.device == DeviceState::Unprovisioned
            )
        },
        "attempt-exhaustion-rollback",
    )?;
    println!("HIL provisioning rollback restored unprovisioned state");
    Ok(())
}

fn hil_reset_xiao_esp32c6(port_path: &str) -> Result<(), String> {
    command(
        "espflash",
        ["reset", "--chip", "esp32c6", "--port", port_path],
    )?;
    let mut port = open_serial_after_reset(port_path)?;
    let mut decoder = SerialFrameDecoder::new();
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::FactoryReset {
                request_id: request_id(1),
            },
        )?,
        1,
        |kind| matches!(kind, ResponseKind::FactoryReset),
        "factory-reset",
    )?;
    println!("HIL provisioning factory reset completed; device reboot expected");

    drop(port);
    thread::sleep(Duration::from_secs(1));
    let mut port = open_serial_after_reset(port_path)?;
    let mut decoder = SerialFrameDecoder::new();
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Status {
                request_id: request_id(2),
            },
        )?,
        2,
        |kind| {
            matches!(
                kind,
                ResponseKind::Status(status)
                    if status.device == DeviceState::Unprovisioned
            )
        },
        "post-reset-status",
    )?;
    println!("HIL provisioning post-reset state is unprovisioned");
    Ok(())
}

fn hil_negative_xiao_esp32c6(port_path: &str) -> Result<(), String> {
    const OPEN_TEST_CONFIGURATION: &[u8] = b"XCF1\0\0\x04\0\0wifi";
    const MALFORMED_CONFIGURATION: &[u8] = b"XCF1";
    const INVALID_CONFIGURATION: &[u8] = b"XCF1\0\0\x04\x01\0wifi";

    command(
        "espflash",
        ["reset", "--chip", "esp32c6", "--port", port_path],
    )?;
    let mut port = open_serial_after_reset(port_path)?;
    let mut decoder = SerialFrameDecoder::new();
    let first_transaction = transaction_id(10);

    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Status {
                request_id: request_id(1),
            },
        )?,
        1,
        |kind| {
            matches!(
                kind,
                ResponseKind::Status(status)
                    if status.device == DeviceState::Unprovisioned
                        && status.transaction == TransactionState::Idle
            )
        },
        "negative-initial-state",
    )?;
    expect_error(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Commit {
                request_id: request_id(2),
                transaction_id: first_transaction,
            },
        )?,
        2,
        ErrorKind::InvalidTransition,
        "reordered-commit",
    )?;

    let begin = || Request::Begin {
        request_id: request_id(3),
        transaction_id: first_transaction,
        schema: CURRENT_SCHEMA,
    };
    expect_response(
        exchange(port.as_mut(), &mut decoder, begin())?,
        3,
        |kind| matches!(kind, ResponseKind::TransactionBegun),
        "begin-for-replay",
    )?;
    expect_response(
        exchange(port.as_mut(), &mut decoder, begin())?,
        3,
        |kind| matches!(kind, ResponseKind::TransactionBegun),
        "duplicate-request-replay",
    )?;
    expect_error(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Begin {
                request_id: request_id(3),
                transaction_id: transaction_id(11),
                schema: CURRENT_SCHEMA,
            },
        )?,
        3,
        ErrorKind::RequestConflict,
        "request-id-conflict",
    )?;

    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::SubmitCandidate {
                request_id: request_id(4),
                transaction_id: first_transaction,
                encoded: MALFORMED_CONFIGURATION,
            },
        )?,
        4,
        |kind| matches!(kind, ResponseKind::CandidateReceived),
        "malformed-submit",
    )?;
    expect_error(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Validate {
                request_id: request_id(5),
                transaction_id: first_transaction,
            },
        )?,
        5,
        ErrorKind::CandidateDecode,
        "malformed-validation",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Status {
                request_id: request_id(6),
            },
        )?,
        6,
        |kind| {
            matches!(
                kind,
                ResponseKind::Status(status)
                    if status.device == DeviceState::Unprovisioned
                        && matches!(status.transaction, TransactionState::CandidateReceived { .. })
            )
        },
        "malformed-no-durable-mutation",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Abort {
                request_id: request_id(7),
                transaction_id: first_transaction,
            },
        )?,
        7,
        |kind| matches!(kind, ResponseKind::Aborted),
        "malformed-abort",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Abort {
                request_id: request_id(7),
                transaction_id: first_transaction,
            },
        )?,
        7,
        |kind| matches!(kind, ResponseKind::Aborted),
        "duplicate-abort-replay",
    )?;

    let second_transaction = transaction_id(20);
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Begin {
                request_id: request_id(8),
                transaction_id: second_transaction,
                schema: CURRENT_SCHEMA,
            },
        )?,
        8,
        |kind| matches!(kind, ResponseKind::TransactionBegun),
        "begin-invalid-candidate",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::SubmitCandidate {
                request_id: request_id(9),
                transaction_id: second_transaction,
                encoded: INVALID_CONFIGURATION,
            },
        )?,
        9,
        |kind| matches!(kind, ResponseKind::CandidateReceived),
        "invalid-submit",
    )?;
    expect_error(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Validate {
                request_id: request_id(10),
                transaction_id: second_transaction,
            },
        )?,
        10,
        ErrorKind::CandidateValidation,
        "invalid-validation",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Abort {
                request_id: request_id(11),
                transaction_id: second_transaction,
            },
        )?,
        11,
        |kind| matches!(kind, ResponseKind::Aborted),
        "invalid-abort",
    )?;

    let disconnect_transaction = transaction_id(30);
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Begin {
                request_id: request_id(12),
                transaction_id: disconnect_transaction,
                schema: CURRENT_SCHEMA,
            },
        )?,
        12,
        |kind| matches!(kind, ResponseKind::TransactionBegun),
        "begin-before-disconnect",
    )?;
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::SubmitCandidate {
                request_id: request_id(13),
                transaction_id: disconnect_transaction,
                encoded: OPEN_TEST_CONFIGURATION,
            },
        )?,
        13,
        |kind| matches!(kind, ResponseKind::CandidateReceived),
        "submit-before-disconnect",
    )?;

    drop(port);
    thread::sleep(Duration::from_millis(5_250));
    let mut port = open_serial_after_reset(port_path)?;
    let mut decoder = SerialFrameDecoder::new();
    expect_response(
        exchange(
            port.as_mut(),
            &mut decoder,
            Request::Status {
                request_id: request_id(14),
            },
        )?,
        14,
        |kind| {
            matches!(
                kind,
                ResponseKind::Status(status)
                    if status.device == DeviceState::Unprovisioned
                        && status.transaction == TransactionState::Idle
            )
        },
        "disconnect-timeout-cleanup",
    )?;
    println!("HIL provisioning negative matrix preserved unprovisioned durable state");
    Ok(())
}

fn open_serial_after_reset(port_path: &str) -> Result<Box<dyn SerialPort>, String> {
    let started = Instant::now();
    loop {
        match serialport::new(port_path, 115_200)
            .timeout(Duration::from_millis(100))
            .open()
        {
            Ok(port) => return Ok(port),
            Err(error) if started.elapsed() < Duration::from_secs(3) => {
                let _ = error;
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(format!(
                    "failed to open fixture serial port {port_path}: {error}"
                ));
            }
        }
    }
}

fn exchange(
    port: &mut dyn SerialPort,
    decoder: &mut SerialFrameDecoder,
    request: Request<'_>,
) -> Result<WireResponse, String> {
    let expected_request_id = request.request_id();
    let mut envelope = [0_u8; MAX_REQUEST_BYTES];
    let mut frame = [0_u8; MAX_TRANSPORT_FRAME_BYTES];
    let send_result = (|| {
        let envelope_len = encode_request(&request, &mut envelope)
            .map_err(|error| format!("request encoding failed: {error:?}"))?;
        let frame_len = encode_serial_frame(
            SerialFrameKind::Request,
            &envelope[..envelope_len],
            &mut frame,
        )
        .map_err(|error| format!("request framing failed: {error:?}"))?;
        port.write_all(&frame[..frame_len])
            .map_err(|error| format!("fixture serial write failed: {error}"))?;
        port.flush()
            .map_err(|error| format!("fixture serial flush failed: {error}"))
    })();
    envelope.zeroize();
    frame.zeroize();
    send_result?;

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut input = [0_u8; 64];
    while Instant::now() < deadline {
        let count = match port.read(&mut input) {
            Ok(count) => count,
            Err(error) if error.kind() == IoErrorKind::TimedOut => continue,
            Err(error) => {
                decoder.clear();
                input.zeroize();
                return Err(format!("fixture serial read failed: {error}"));
            }
        };
        for &byte in &input[..count] {
            let serial_frame = match decoder.push(byte) {
                Ok(Some(frame)) => frame,
                Ok(None) | Err(_) => continue,
            };
            if serial_frame.kind() != SerialFrameKind::Response {
                decoder.clear();
                continue;
            }
            let response = match decode_response(serial_frame.payload()) {
                Ok(response) => response,
                Err(error) => {
                    decoder.clear();
                    input.zeroize();
                    return Err(format!("response decoding failed: {error:?}"));
                }
            };
            decoder.clear();
            input.zeroize();
            if wire_request_id(response) != expected_request_id {
                return Err("response request identifier mismatch".to_owned());
            }
            return Ok(response);
        }
        input.zeroize();
    }
    decoder.clear();
    input.zeroize();
    Err("timed out waiting for fixture response".to_owned())
}

fn expect_response(
    response: WireResponse,
    expected_request_id: u32,
    expected_kind: impl FnOnce(ResponseKind) -> bool,
    operation: &str,
) -> Result<(), String> {
    match response {
        WireResponse::Success(response)
            if response.request_id.get() == expected_request_id && expected_kind(response.kind) =>
        {
            println!("HIL provisioning {operation}: ok");
            Ok(())
        }
        WireResponse::Success(_) => Err(format!(
            "HIL provisioning {operation} returned an unexpected response"
        )),
        WireResponse::Error(error) => Err(format!(
            "HIL provisioning {operation} failed: {:?}",
            error.kind
        )),
    }
}

fn expect_error(
    response: WireResponse,
    expected_request_id: u32,
    expected_kind: ErrorKind,
    operation: &str,
) -> Result<(), String> {
    match response {
        WireResponse::Error(error)
            if error.request_id.get() == expected_request_id && error.kind == expected_kind =>
        {
            println!("HIL provisioning {operation}: ok");
            Ok(())
        }
        WireResponse::Error(_) => Err(format!(
            "HIL provisioning {operation} returned an unexpected error"
        )),
        WireResponse::Success(_) => Err(format!(
            "HIL provisioning {operation} unexpectedly succeeded"
        )),
    }
}

fn request_id(value: u32) -> RequestId {
    RequestId::new(value).expect("constant request ID is nonzero")
}

fn transaction_id(value: u64) -> TransactionId {
    TransactionId::new(value).expect("constant transaction ID is nonzero")
}

fn wire_request_id(response: WireResponse) -> RequestId {
    match response {
        WireResponse::Success(response) => response.request_id,
        WireResponse::Error(error) => error.request_id,
    }
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
    println!("  cargo xtask hil-smoke-xiao-esp32c6 <port>  run non-secret serial smoke flow");
    println!("  cargo xtask hil-reset-xiao-esp32c6 <port>  request fixture factory reset");
    println!(
        "  cargo xtask hil-negative-xiao-esp32c6 <port>  run non-secret serial negative matrix"
    );
    println!("  cargo xtask build-xiao-esp32c6  build release firmware");
    println!("  cargo xtask build-xiao-esp32c6-hil  build fixture provisioning firmware");
    println!("  cargo xtask run-xiao-esp32c6    build, flash, and monitor firmware");
    println!("  cargo xtask build-xiao-esp32c6-beacon  build iBeacon firmware");
    println!("  cargo xtask run-xiao-esp32c6-beacon    build, flash, and monitor iBeacon");
    println!("  cargo xtask build-xiao-esp32c6-beacon-scanner  build BLE scanner firmware");
    println!(
        "  cargo xtask run-xiao-esp32c6-beacon-scanner    build, flash, and monitor BLE scanner"
    );
}
