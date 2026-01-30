# Agent Instructions (atlas)

This file guides automated agents working in the `atlas` workspace. Follow the referenced sources and conventions below.

## Canonical references

- Copilot guidance: `.copilot/copilot-instructions.md`
- Toolchain pinning: `rust-toolchain.toml`
- CI workflows:
  - `.github/workflows/ci.yml`
  - `.github/workflows/ci-conditional.yml`
  - `.github/workflows/release.yml`
  - `.github/workflows/pages.yml`

## Rust conventions (adopted)

- Edition: Rust 2024 (see crate `Cargo.toml` files).
- Formatting: `cargo fmt --workspace` (or `cargo fmt --all`).
- Linting: `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- Workspace structure: crates live under `crates/` and are narrowly scoped.
- Public API: expose from each crate's `src/lib.rs`; keep internal modules in `src/`.
- Design: follow single-responsibility; keep functions focused and files small.
- Modules: use submodules in separate files rather than long monolithic files.

## Naming conventions (adopted)

- Crates and modules: `snake_case` (e.g., `web`, `runtime`).
- Types and traits: `PascalCase`.
- Functions and methods: `snake_case`.
- Constants and statics: `SCREAMING_SNAKE_CASE`.
- File names: `snake_case.rs`.

## CI expectations

- CI runs format checks, clippy, `cargo check`, and tests across the workspace.
- Conditional CI runs the same checks when code/doc/Cargo files change.

## Preferred commands

- Format: `cargo fmt --workspace`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Check: `cargo check --workspace --all-targets`
- Test: `cargo test --workspace --all-features --no-fail-fast`
