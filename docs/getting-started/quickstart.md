# Quickstart

## Build (native viewer)
1. Install Rust stable and required toolchains
2. Build:
   - `cargo build -p viewer_native`
3. Run:
   - `cargo run -p viewer_native`

## Build (web viewer)
1. Install wasm target:
   - `rustup target add wasm32-unknown-unknown`
2. Build:
   - `cargo build -p viewer_web --target wasm32-unknown-unknown`

## Load a scene
- Scenes are loaded from a scene package (`.scn` or directory-based package).
- The viewer can load local packages and later remote packages (HTTP range planned).

## Verify determinism
- Run the same scene twice with the same input replay.
- Output should match (render outputs may differ by GPU, semantic results must match).
