# Embedded SDK

A minimal, dependency-free Rust SDK foundation for embedded devices.

The library supports `no_std` targets:

```rust
let greeting = embedded_sdk::hello();
```

Run the included host example:

```sh
cargo run --example hello
```

Run the tests:

```sh
cargo test
```
