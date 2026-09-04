//! Repository automation discovered from Cargo package metadata.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    process::{Command, ExitCode, Stdio},
};

use cargo_metadata::{MetadataCommand, Package};
use clap::{Parser, Subcommand};
use serde::Deserialize;

const METADATA_NAMESPACE: &str = "embedded-sdk";

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "Repository automation for embedded-sdk",
    after_help = "Firmware selectors use BOARD for its default firmware and BOARD/VARIANT for other variants.\nRun `cargo xtask list` to see the available selectors."
)]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build one release firmware image.
    Build {
        /// Firmware selector or Cargo package name.
        firmware: String,
    },
    /// Build every registered release firmware image.
    BuildAll {
        /// Limit builds to one board.
        #[arg(long)]
        board: Option<String>,
    },
    /// Format, lint, and test host packages.
    Check,
    /// Verify Rust targets and flash tools for registered boards.
    Doctor {
        /// Board id; all boards are checked when omitted.
        board: Option<String>,
    },
    /// List registered boards and firmware images.
    List,
    /// Build, flash, and monitor one firmware image.
    Run {
        /// Firmware selector or Cargo package name.
        firmware: String,
    },
    /// Run host tests.
    Test,
    /// Print registered Rust targets, one per line.
    Targets,
}

#[derive(Clone, Debug, Deserialize)]
struct BoardMetadata {
    id: String,
    target: String,
    runner: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct FirmwareMetadata {
    board: String,
    variant: String,
}

#[derive(Clone, Debug)]
struct Board {
    package: String,
    metadata: BoardMetadata,
}

#[derive(Clone, Debug)]
struct Firmware {
    package: String,
    board: String,
    variant: String,
    selector: String,
}

#[derive(Debug)]
struct Registry {
    boards: BTreeMap<String, Board>,
    firmware: Vec<Firmware>,
    host_exclusions: BTreeSet<String>,
}

impl Registry {
    fn discover() -> Result<Self, String> {
        let cargo_metadata = MetadataCommand::new()
            .no_deps()
            .exec()
            .map_err(|error| format!("failed to read Cargo workspace metadata: {error}"))?;

        Self::from_packages(&cargo_metadata.packages)
    }

    fn from_packages(packages: &[Package]) -> Result<Self, String> {
        let mut boards = BTreeMap::new();
        let mut firmware_metadata = Vec::new();
        let mut host_exclusions = BTreeSet::new();

        for package in packages {
            let Some(namespace) = package.metadata.get(METADATA_NAMESPACE) else {
                continue;
            };

            if namespace
                .get("host-check")
                .and_then(serde_json::Value::as_bool)
                == Some(false)
            {
                host_exclusions.insert(package.name.clone());
            }

            if let Some(value) = namespace.get("board") {
                let metadata: BoardMetadata = parse_metadata(package, "board", value)?;
                validate_board_metadata(package, &metadata)?;
                let id = metadata.id.clone();
                let board = Board {
                    package: package.name.clone(),
                    metadata,
                };
                if let Some(existing) = boards.insert(id.clone(), board) {
                    return Err(format!(
                        "board id {id:?} is declared by both {} and {}",
                        existing.package, package.name
                    ));
                }
            }

            if let Some(value) = namespace.get("firmware") {
                let metadata: FirmwareMetadata = parse_metadata(package, "firmware", value)?;
                firmware_metadata.push((package.name.clone(), metadata));
                host_exclusions.insert(package.name.clone());
            }
        }

        let mut firmware = Vec::with_capacity(firmware_metadata.len());
        let mut selectors = BTreeMap::new();
        for (package, metadata) in firmware_metadata {
            if !boards.contains_key(&metadata.board) {
                return Err(format!(
                    "firmware package {package} references unknown board {:?}",
                    metadata.board
                ));
            }
            if metadata.variant.is_empty() || metadata.variant.contains('/') {
                return Err(format!(
                    "firmware package {package} has invalid variant {:?}",
                    metadata.variant
                ));
            }

            let selector = firmware_selector(&metadata.board, &metadata.variant);
            if let Some(existing) = selectors.insert(selector.clone(), package.clone()) {
                return Err(format!(
                    "firmware selector {selector:?} is declared by both {existing} and {package}"
                ));
            }
            firmware.push(Firmware {
                package,
                board: metadata.board,
                variant: metadata.variant,
                selector,
            });
        }
        firmware.sort_by(|left, right| left.selector.cmp(&right.selector));

        Ok(Self {
            boards,
            firmware,
            host_exclusions,
        })
    }

    fn board(&self, id_or_package: &str) -> Result<&Board, String> {
        self.boards
            .get(id_or_package)
            .or_else(|| {
                self.boards
                    .values()
                    .find(|board| board.package == id_or_package)
            })
            .ok_or_else(|| {
                format!(
                    "unknown board {id_or_package:?}; run `cargo xtask list` to see registered boards"
                )
            })
    }

    fn firmware(&self, selector_or_package: &str) -> Result<&Firmware, String> {
        self.firmware
            .iter()
            .find(|firmware| {
                firmware.selector == selector_or_package || firmware.package == selector_or_package
            })
            .ok_or_else(|| {
                format!(
                    "unknown firmware {selector_or_package:?}; run `cargo xtask list` to see registered firmware"
                )
            })
    }
}

fn main() -> ExitCode {
    let (args, legacy_command) = normalize_legacy_args(env::args_os());
    let cli = Cli::parse_from(args);

    if let Some(command) = legacy_command {
        eprintln!(
            "warning: `cargo xtask {command}` is deprecated; use the generic subcommand form"
        );
    }

    let result = execute(cli.command);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(task: Task) -> Result<(), String> {
    match task {
        Task::Check => check(&Registry::discover()?),
        Task::Test => host_tests(&Registry::discover()?),
        Task::List => list(&Registry::discover()?),
        Task::Build { firmware } => build(&Registry::discover()?, &firmware),
        Task::BuildAll { board } => build_all(&Registry::discover()?, board.as_deref()),
        Task::Doctor { board } => doctor(&Registry::discover()?, board.as_deref()),
        Task::Run { firmware } => run(&Registry::discover()?, &firmware),
        Task::Targets => targets(&Registry::discover()?),
    }
}

fn normalize_legacy_args(
    args: impl IntoIterator<Item = OsString>,
) -> (Vec<OsString>, Option<String>) {
    let mut args: Vec<_> = args.into_iter().collect();
    let Some(command) = args
        .get(1)
        .and_then(|argument| argument.to_str())
        .map(str::to_owned)
    else {
        return (args, None);
    };

    let replacement = if command != "build-all" {
        command
            .strip_prefix("build-")
            .map(|firmware| ("build", firmware))
    } else {
        None
    }
    .or_else(|| {
        command
            .strip_prefix("run-")
            .map(|firmware| ("run", firmware))
    });

    let Some((subcommand, firmware)) = replacement else {
        return (args, None);
    };

    let legacy_command = command.clone();
    args[1] = subcommand.into();
    args.insert(2, firmware.into());
    (args, Some(legacy_command))
}

fn parse_metadata<T: for<'de> Deserialize<'de>>(
    package: &Package,
    kind: &str,
    value: &serde_json::Value,
) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| {
        format!(
            "invalid {kind} metadata in package {}: {error}",
            package.name
        )
    })
}

fn validate_board_metadata(package: &Package, metadata: &BoardMetadata) -> Result<(), String> {
    if metadata.id.is_empty() || metadata.id.contains('/') {
        return Err(format!(
            "board package {} has invalid id {:?}",
            package.name, metadata.id
        ));
    }
    if metadata.target.is_empty() {
        return Err(format!(
            "board package {} has an empty target",
            package.name
        ));
    }
    if metadata.runner.is_empty() || metadata.runner.iter().any(String::is_empty) {
        return Err(format!(
            "board package {} must declare a non-empty runner command",
            package.name
        ));
    }
    if metadata
        .runner
        .iter()
        .any(|component| component.chars().any(char::is_whitespace))
    {
        return Err(format!(
            "board package {} runner components cannot contain whitespace",
            package.name
        ));
    }
    Ok(())
}

fn firmware_selector(board: &str, variant: &str) -> String {
    if variant == "default" {
        board.to_owned()
    } else {
        format!("{board}/{variant}")
    }
}

fn build(registry: &Registry, selector: &str) -> Result<(), String> {
    let firmware = registry.firmware(selector)?;
    let board = registry.board(&firmware.board)?;
    build_firmware(firmware, board)
}

fn build_all(registry: &Registry, board_filter: Option<&str>) -> Result<(), String> {
    let board_filter = board_filter
        .map(|board| {
            registry
                .board(board)
                .map(|board| board.metadata.id.as_str())
        })
        .transpose()?;
    let selected: Vec<_> = registry
        .firmware
        .iter()
        .filter(|firmware| board_filter.is_none_or(|board| firmware.board == board))
        .collect();

    if selected.is_empty() {
        return Err("no registered firmware matched the requested board".to_owned());
    }

    for firmware in selected {
        let board = registry.board(&firmware.board)?;
        println!("Building {} ({})", firmware.selector, firmware.package);
        build_firmware(firmware, board)?;
    }
    Ok(())
}

fn build_firmware(firmware: &Firmware, board: &Board) -> Result<(), String> {
    cargo([
        "build",
        "-p",
        firmware.package.as_str(),
        "--target",
        board.metadata.target.as_str(),
        "--release",
    ])
}

fn run(registry: &Registry, selector: &str) -> Result<(), String> {
    let firmware = registry.firmware(selector)?;
    let board = registry.board(&firmware.board)?;
    let runner_variable = cargo_runner_variable(&board.metadata.target);
    let runner = board.metadata.runner.join(" ");
    let executable = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    command_with_env(
        executable,
        [
            "run",
            "-p",
            firmware.package.as_str(),
            "--target",
            board.metadata.target.as_str(),
            "--release",
        ],
        [(runner_variable, runner)],
    )
}

fn cargo_runner_variable(target: &str) -> String {
    format!(
        "CARGO_TARGET_{}_RUNNER",
        target.replace('-', "_").to_ascii_uppercase()
    )
}

fn check(registry: &Registry) -> Result<(), String> {
    cargo(["fmt", "--all", "--check"])?;

    let mut args = vec!["clippy".to_owned(), "--workspace".to_owned()];
    add_exclusions(&mut args, &registry.host_exclusions);
    args.extend(
        ["--all-targets", "--", "-D", "warnings"]
            .into_iter()
            .map(str::to_owned),
    );
    cargo(args)?;
    host_tests(registry)
}

fn host_tests(registry: &Registry) -> Result<(), String> {
    let mut args = vec!["test".to_owned(), "--workspace".to_owned()];
    add_exclusions(&mut args, &registry.host_exclusions);
    cargo(args)
}

fn add_exclusions(args: &mut Vec<String>, exclusions: &BTreeSet<String>) {
    for package in exclusions {
        args.push("--exclude".to_owned());
        args.push(package.clone());
    }
}

fn doctor(registry: &Registry, board_filter: Option<&str>) -> Result<(), String> {
    let selected: Vec<_> = match board_filter {
        Some(board) => vec![registry.board(board)?],
        None => registry.boards.values().collect(),
    };
    if selected.is_empty() {
        return Err("no boards are registered".to_owned());
    }

    let installed = output("rustup", ["target", "list", "--installed"])?;
    let mut checked_targets = BTreeSet::new();
    let mut checked_tools = BTreeSet::new();
    for board in selected {
        let target = board.metadata.target.as_str();
        if checked_targets.insert(target) && !installed.lines().any(|installed| installed == target)
        {
            return Err(format!(
                "missing Rust target {target}; run `rustup target add {target}`"
            ));
        }

        let tool = &board.metadata.runner[0];
        if checked_tools.insert(tool.as_str()) && command(tool, ["--version"]).is_err() {
            return Err(format!("runner {tool} is unavailable"));
        }

        println!(
            "{}: target={}, runner={}",
            board.metadata.id,
            target,
            board.metadata.runner.join(" ")
        );
    }
    println!("Development environment is ready");
    Ok(())
}

fn list(registry: &Registry) -> Result<(), String> {
    if registry.boards.is_empty() {
        println!("No boards are registered.");
        return Ok(());
    }

    println!("Boards:");
    for board in registry.boards.values() {
        println!(
            "  {:<24} {:<32} {}",
            board.metadata.id, board.metadata.target, board.package
        );
    }

    println!();
    println!("Firmware:");
    for firmware in &registry.firmware {
        println!(
            "  {:<32} {:<20} {}",
            firmware.selector, firmware.variant, firmware.package
        );
    }
    Ok(())
}

fn targets(registry: &Registry) -> Result<(), String> {
    let targets: BTreeSet<_> = registry
        .boards
        .values()
        .map(|board| board.metadata.target.as_str())
        .collect();
    if targets.is_empty() {
        return Err("no board targets are registered".to_owned());
    }
    for target in targets {
        println!("{target}");
    }
    Ok(())
}

fn cargo<I, S>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let executable = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    command(executable, args)
}

fn command<I, S>(program: impl AsRef<OsStr>, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    command_with_env(program, args, std::iter::empty::<(String, String)>())
}

fn command_with_env<I, S, E, K, V>(
    program: impl AsRef<OsStr>,
    args: I,
    environment: E,
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let program = program.as_ref();
    let status = Command::new(program)
        .args(args)
        .envs(environment)
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

#[cfg(test)]
mod tests {
    use super::{cargo_runner_variable, firmware_selector, normalize_legacy_args};
    use std::ffi::OsString;

    #[test]
    fn default_firmware_uses_the_board_as_its_selector() {
        assert_eq!(firmware_selector("xiao-esp32c6", "default"), "xiao-esp32c6");
        assert_eq!(
            firmware_selector("xiao-esp32c6", "beacon"),
            "xiao-esp32c6/beacon"
        );
    }

    #[test]
    fn legacy_firmware_commands_are_translated_generically() {
        let (args, legacy) = normalize_legacy_args(
            ["xtask", "build-xiao-esp32c6-beacon"]
                .into_iter()
                .map(OsString::from),
        );
        assert_eq!(
            args,
            ["xtask", "build", "xiao-esp32c6-beacon"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(legacy.as_deref(), Some("build-xiao-esp32c6-beacon"));
    }

    #[test]
    fn build_all_is_not_treated_as_a_legacy_firmware_command() {
        let (args, legacy) =
            normalize_legacy_args(["xtask", "build-all"].into_iter().map(OsString::from));
        assert_eq!(args, [OsString::from("xtask"), OsString::from("build-all")]);
        assert_eq!(legacy, None);
    }

    #[test]
    fn cargo_runner_environment_variable_is_derived_from_target() {
        assert_eq!(
            cargo_runner_variable("riscv32imac-unknown-none-elf"),
            "CARGO_TARGET_RISCV32IMAC_UNKNOWN_NONE_ELF_RUNNER"
        );
    }
}
