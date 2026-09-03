# Contributing

## Required checks

Run the host quality gate before opening a change:

```sh
cargo xtask check
```

Changes affecting Espressif code must also build the ESP32-C6 reference
firmware:

```sh
cargo xtask build-xiao-esp32c6
cargo xtask build-beetle-esp32c6-battery
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

## Dependency changes

Explain why a dependency is needed, disable unnecessary default features, and
prefer a stable crates.io release. Git dependencies require a pinned revision,
an owner, and an exit plan.
