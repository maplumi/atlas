# Copilot Instructions — atlas

This file documents the expected programming paradigms, code organization, formatting/linting workflow, and testing strategy for the `atlas` workspace. Refer to the overall project governance and high-level direction in the preamble: [docs/preamble.md](docs/preamble.md).

**Programming paradigms**
- Prefer idiomatic Rust: ownership, borrowing, and explicit `Result`/`Option` handling.
- Small, pure functions where possible; keep side-effects isolated to boundary layers (I/O, GPU, network).
- Favor composition over inheritance: use traits and small concrete types.
- Use zero-cost abstractions and iterator combinators for clarity and performance.
- Keep mutability local and explicit; prefer `&` and `&mut` semantics over cloning.
- Error handling: return `Result<T, E>` from fallible APIs; provide clear error types and use `thiserror` or similar for convenience.

**Code organization**
- Project is a Cargo workspace; each crate under `crates/` represents a logical module (e.g., `formats`, `gpu`, `layers`). Keep crate responsibilities narrow and well-defined.
- Within a crate:
  - `src/lib.rs` should expose the public API; use module files under `src/` to implement internals.
  - Keep small modules (single responsibility) and prefer `mod foo;` with `foo.rs` over very large files.
  - Re-export important types at the crate root when they are part of the public API (e.g., `pub use crate::module::Type;`).
  - Private helper modules should live in `mod` files and be hidden behind `pub(crate)` or `pub(super)` as appropriate.
- Tests:
  - Unit tests: colocate with code in `#[cfg(test)] mod tests { ... }` inside the same module file.
  - Integration tests: place in the workspace `tests/` directory (or crate-level `tests/`) and treat them as black-box usage of the crate's public API.
  - Doc tests: keep them meaningful and small — they serve as living documentation.

**Formatting & linting (local workflow)**
- Use `rustfmt` and `clippy` on every change. Recommended commands:

  - `cargo fmt --workspace` — format all workspace crates.
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings` — run clippy and deny warnings.
  - `cargo check --workspace` — quick compilation check.

- Make `cargo fmt` non-optional: developers should run it before committing. Consider a pre-commit hook to run `cargo fmt` and `cargo clippy`.

**Testing strategy (recommended Rust standards)**
- Unit tests
  - Place unit tests next to the code they validate using `#[cfg(test)]` and `#[test]`.
  - Keep tests small, deterministic, and fast. Mock or stub external systems where appropriate.

- Integration tests
  - Put integration tests in the `tests/` directory at workspace root or crate-level `tests/` directories.
  - Use these to validate the public API and cross-crate interactions.
  - Extract shared test helpers to `tests/common.rs` or a `tests/common` crate when multiple tests need them.

- Doc tests
  - Use doc comments for short examples. Keep doc examples minimal and ensure they compile with `cargo test`.

- Test features and CI
  - Run `cargo test --workspace` locally. For coverage of optional features, run `cargo test --workspace --all-features` in CI.
  - For slow or platform-dependent tests, mark them with `#[ignore]` and run separately in appropriate environments.

- Concurrency and flakiness
  - Prefer deterministic tests. If tests require ordering or global state, run those tests with `--test-threads=1` when necessary.

**CI / pre-merge checks (recommended)**
- CI should run the following steps on PRs and merges:
  1. `cargo fmt --all -- --check` — verify formatting.
  2. `cargo clippy --workspace --all-targets --all-features -- -D warnings` — static linting.
  3. `cargo check --workspace --all-targets` — compilation checks.
  4. `cargo test --workspace --all-features` — tests across the workspace.

**Commit / code review expectations**
- Commits should be small and focused. Each PR should have a clear purpose and link to relevant docs or issues.
- Include brief description of test coverage added or changed.
- If the change touches public APIs, document the change in `CHANGELOG.md` or the crate's readme as appropriate.

**Developer tips & commands**
- Format everything: `cargo fmt --workspace`
- Lint everything: `cargo clippy --workspace --all-targets -- -D warnings`
- Quick compile check: `cargo check --workspace`
- Run tests: `cargo test --workspace`
- Run a single crate tests: `cargo test -p crate_name`

**Where to look for project intent**
Refer to the high-level governance and project purpose in the preamble: [docs/preamble.md](docs/preamble.md).

---
If you want, I can also add a simple GitHub Actions CI workflow that runs the CI checklist above, and/or add a sample `pre-commit` hook for `cargo fmt` and `cargo clippy`.
