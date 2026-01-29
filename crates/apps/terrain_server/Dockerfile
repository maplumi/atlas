# syntax=docker/dockerfile:1

FROM rust:1.93 as builder
WORKDIR /app

# Build in workspace context
COPY Cargo.toml Cargo.toml
COPY crates crates
RUN cargo build -p terrain_server --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/terrain_server /usr/local/bin/terrain_server
EXPOSE 9100
ENV TERRAIN_ADDR=0.0.0.0:9100
ENV TERRAIN_ROOT=/data/terrain
CMD ["/usr/local/bin/terrain_server"]
