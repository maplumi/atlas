#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/release.sh <bump>
# bump: major | minor | patch

bump=${1:-patch}
echo "Preparing release (bump=$bump)"

if [ -z "${GITHUB_REPOSITORY:-}" ] || [ -z "${GITHUB_TOKEN:-}" ]; then
  echo "GITHUB_REPOSITORY and GITHUB_TOKEN must be set in CI environment" >&2
  exit 1
fi

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

echo "Setting remote URL with token for push"
git remote set-url origin "https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"

echo "Installing cargo-release"
cargo install cargo-release --force

echo "Running cargo release $bump (will create tag and changelog)"
# --no-publish prevents crates.io publish; --execute applies changes; --no-confirm disables prompts
cargo release --no-publish --execute --no-confirm "$bump"

echo "Release prep complete"
