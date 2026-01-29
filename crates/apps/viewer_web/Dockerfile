# syntax=docker/dockerfile:1

FROM rust:1.93 as builder
WORKDIR /app

RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk --locked

COPY Cargo.toml Cargo.toml
COPY crates/apps/viewer_web/Cargo.toml crates/apps/viewer_web/Cargo.toml
COPY crates/apps/viewer_native/Cargo.toml crates/apps/viewer_native/Cargo.toml
COPY crates/apps/terrain_server/Cargo.toml crates/apps/terrain_server/Cargo.toml
COPY crates/apps/terrain_server/src crates/apps/terrain_server/src
COPY crates/apps/viewer_web/Trunk.toml crates/apps/viewer_web/Trunk.toml
COPY crates/apps/viewer_web/index.html crates/apps/viewer_web/index.html
COPY crates/apps/viewer_web/assets crates/apps/viewer_web/assets
COPY crates/apps/viewer_web/src crates/apps/viewer_web/src
COPY crates/foundation crates/foundation
COPY crates/runtime crates/runtime
COPY crates/scene crates/scene
COPY crates/streaming crates/streaming
COPY crates/catalog crates/catalog
COPY crates/formats crates/formats
COPY crates/gpu crates/gpu
COPY crates/layers crates/layers
COPY crates/compute crates/compute
COPY crates/tools crates/tools

WORKDIR /app/crates/apps/viewer_web
RUN trunk build --release

FROM nginx:alpine
COPY --from=builder /app/crates/apps/viewer_web/dist /usr/share/nginx/html
EXPOSE 80
