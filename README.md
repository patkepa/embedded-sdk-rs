# Embedded SDK

An Embassy-first, `no_std` Rust workspace for building connected embedded
devices across hardware platforms.

## Repository layout

```text
crates/       portable SDK libraries
ports/        chip-family and runtime integration
boards/       physical board support packages
firmware/     deployable product and reference binaries
tests/        host, integration, and hardware test suites
tools/xtask/  repository automation
docs/         architecture, porting, and support documentation
```

See [Repository Architecture](docs/architecture/repository-structure.md) for
the intended long-term structure and dependency rules.


802.15.4/OpenThread, cloud connectivity, storage, and OTA remain planned.

## License

Licensed under either Apache License 2.0 or the MIT license, at your option.
