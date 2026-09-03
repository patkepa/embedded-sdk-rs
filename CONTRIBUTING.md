# Contributing

## Required checks

Run the host quality gate before opening a change:

```sh
cargo xtask check
```

Changes affecting Espressif code must also build the ESP32-C6 reference
firmware:

```sh
cargo xtask build xiao-esp32c6
```

Hardware behavior changes should include hardware-in-the-loop evidence when a
test fixture is available.

## Architecture rules

- Portable crates are `no_std` by default.
- Prefer `embedded-hal`, `embedded-hal-async`, and `embedded-io` traits.
- Portable crates must not depend on vendor HALs or board packages.
- A board owns physical wiring; firmware owns product policy.
- Cargo features must be additive.
- Unsafe Rust is forbidden unless an architecture decision explicitly creates
  and reviews an isolated FFI or platform boundary.
- Public APIs require documentation and tests.

Consequential design decisions belong in `docs/adr/` before implementation.

## Registering boards and firmware

Board and firmware workflows are discovered from Cargo package metadata. Add
the target and runner to each board package:

```toml
[package.metadata.embedded-sdk.board]
id = "board-id"
target = "rust-target-triple"
runner = ["flash-tool", "flash", "--monitor"]
```

Each firmware package then references its board and declares a variant. The
`default` variant uses the board id as its short selector; other variants use
`board-id/variant`:

```toml
[package.metadata.embedded-sdk.firmware]
board = "board-id"
variant = "default"
```

Run `cargo xtask list` to validate the registry and see its selectors. Firmware
packages are automatically excluded from host checks and included by
`cargo xtask build-all`; their targets are emitted by `cargo xtask targets` for
CI setup. No `xtask` source or CI workflow change is required.

## Dependency changes

Explain why a dependency is needed, disable unnecessary default features, and
prefer a stable crates.io release. Git dependencies require a pinned revision,
an owner, and an exit plan.
