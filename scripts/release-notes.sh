#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: $0 <tag>" >&2
  exit 1
fi

tag="$1"
version="${tag#v}"

awk -v tag="$tag" -v version="$version" '
  $0 ~ "^## \\[[^]]+\\]" { if (found) exit }
  $0 ~ "^## \\[" tag "\\]" { found=1 }
  $0 ~ "^## \\[" version "\\]" { found=1 }
  found { print }
' CHANGELOG.md
