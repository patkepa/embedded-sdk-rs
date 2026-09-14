FROM rust:1.96.0-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY ports ports
COPY boards boards
COPY firmware firmware
COPY tests/host tests/host
COPY tools tools
RUN cargo build --locked --release -p embedded-sdk-dev-backend

FROM debian:bookworm-slim
COPY --from=builder /src/target/release/embedded-sdk-dev-backend /usr/local/bin/collector
RUN mkdir /data && chmod 755 /data
ENTRYPOINT ["collector"]
