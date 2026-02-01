# syntax=docker/dockerfile:1

FROM rust:1.93 as builder
WORKDIR /app

RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk --locked
RUN cargo install wasm-bindgen-cli --version 0.2.108 --locked

COPY Cargo.toml Cargo.toml
COPY crates/apps/web/Cargo.toml crates/apps/web/Cargo.toml
COPY crates/apps/native/Cargo.toml crates/apps/native/Cargo.toml
COPY crates/apps/server/Cargo.toml crates/apps/server/Cargo.toml
COPY crates/apps/server/src crates/apps/server/src
COPY crates/apps/web/Trunk.toml crates/apps/web/Trunk.toml
COPY crates/apps/web/index.html crates/apps/web/index.html
COPY crates/apps/web/assets crates/apps/web/assets
COPY crates/apps/web/src crates/apps/web/src
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

WORKDIR /app/crates/apps/web
RUN trunk build --release

FROM nginx:alpine
COPY --from=builder /app/crates/apps/web/dist /usr/share/nginx/html
COPY crates/apps/web/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 9103
