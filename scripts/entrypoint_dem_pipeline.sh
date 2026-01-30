#!/usr/bin/env bash
set -euo pipefail

: "${TERRAIN_ROOT:=/data/terrain}"
: "${TERRAIN_RAW:=/data/terrain/raw}"
: "${STAC_URL:=https://copernicus-dem-30m-stac.s3.amazonaws.com}"
: "${TERRAIN_COLLECTION:?TERRAIN_COLLECTION is required}"
: "${TERRAIN_BBOX:?TERRAIN_BBOX is required}"
: "${TERRAIN_LIMIT:=200}"
: "${TERRAIN_ZOOM_MIN:=0}"
: "${TERRAIN_ZOOM_MAX:=2}"
: "${TERRAIN_TILE_SIZE:=256}"
: "${TERRAIN_SAMPLE_STEP:=4}"
: "${TERRAIN_NO_DATA:=-9999}"
: "${TERRAIN_FORCE_REBUILD:=0}"

mkdir -p "$TERRAIN_RAW" "$TERRAIN_ROOT/metadata" "$TERRAIN_ROOT/tiles"

echo "[dem] download COGs -> $TERRAIN_RAW"
/usr/local/bin/terrain_fetch \
  --stac-url "$STAC_URL" \
  download \
  --collection "$TERRAIN_COLLECTION" \
  --bbox="$TERRAIN_BBOX" \
  --out "$TERRAIN_RAW" \
  --limit "$TERRAIN_LIMIT"

if [[ -f "$TERRAIN_ROOT/metadata/tileset.json" && "$TERRAIN_FORCE_REBUILD" != "1" ]]; then
  echo "[dem] tileset exists; skipping rebuild (set TERRAIN_FORCE_REBUILD=1 to override)"
  exit 0
fi

echo "[dem] build tileset + tiles"
/app/dem_pipeline.py \
  --input "$TERRAIN_RAW" \
  --output "$TERRAIN_ROOT" \
  --bbox "$TERRAIN_BBOX" \
  --zoom-min "$TERRAIN_ZOOM_MIN" \
  --zoom-max "$TERRAIN_ZOOM_MAX" \
  --tile-size "$TERRAIN_TILE_SIZE" \
  --sample-step "$TERRAIN_SAMPLE_STEP" \
  --no-data "$TERRAIN_NO_DATA"
