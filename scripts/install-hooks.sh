#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HOOKS_DIR="$ROOT_DIR/.githooks"

echo "Installing git hooks from $HOOKS_DIR"

if [ ! -d "$HOOKS_DIR" ]; then
  echo "No .githooks directory found at $HOOKS_DIR" >&2
  exit 1
fi

chmod +x "$HOOKS_DIR/pre-commit" || true
git config core.hooksPath .githooks

echo "Hooks installed. To enable for other clones, run this in your clone:" 
echo "  git config core.hooksPath .githooks"
echo "Pre-commit hook will run 'cargo fmt --check' and 'cargo clippy'."
