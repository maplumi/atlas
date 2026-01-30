#!/usr/bin/env python3
import argparse
import json
import math
import os
import subprocess
import sys
from pathlib import Path

GDAL_REQUIRED = ["gdalinfo", "gdalbuildvrt", "gdalwarp", "gdal_translate"]


def run(cmd):
    print("+", " ".join(str(c) for c in cmd))
    subprocess.run(cmd, check=True)


def require_gdal():
    for tool in GDAL_REQUIRED:
        if subprocess.call(["which", tool], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) != 0:
            raise RuntimeError(f"Missing {tool} in PATH. Install GDAL CLI tools first.")


def gdalinfo_stats(path: Path):
    result = subprocess.run(
        ["gdalinfo", "-stats", "-mm", "-json", str(path)],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    data = json.loads(result.stdout)
    bands = data.get("bands") or []
    if not bands:
        return -1000.0, 9000.0
    stats = bands[0].get("metadata", {}).get("", {})
    min_v = stats.get("STATISTICS_MINIMUM")
    max_v = stats.get("STATISTICS_MAXIMUM")
    try:
        return float(min_v), float(max_v)
    except Exception:
        return -1000.0, 9000.0


def tile_bounds(z, x, y):
    return tile_bounds_in_bounds(z, x, y, -180.0, 180.0, -90.0, 90.0)


def tile_bounds_in_bounds(z, x, y, min_lon, max_lon, min_lat, max_lat):
    n = 2 ** z
    lon_span = (max_lon - min_lon) / n
    lat_span = (max_lat - min_lat) / n
    lon_min = min_lon + x * lon_span
    lon_max = lon_min + lon_span
    lat_top = max_lat
    lat_max = lat_top - y * lat_span
    lat_min = lat_max - lat_span
    return lon_min, lon_max, lat_min, lat_max


def parse_bbox(s: str):
    parts = [p.strip() for p in s.split(",")]
    if len(parts) != 4:
        raise ValueError("bbox must be minLon,minLat,maxLon,maxLat")
    min_lon, min_lat, max_lon, max_lat = [float(p) for p in parts]
    if not (math.isfinite(min_lon) and math.isfinite(min_lat) and math.isfinite(max_lon) and math.isfinite(max_lat)):
        raise ValueError("bbox contains non-finite values")
    if max_lon <= min_lon or max_lat <= min_lat:
        raise ValueError("bbox is empty/invalid")
    return min_lon, min_lat, max_lon, max_lat


def tile_range_for_bbox(z: int, bbox_min_lon: float, bbox_min_lat: float, bbox_max_lon: float, bbox_max_lat: float,
                       min_lon: float, max_lon: float, min_lat: float, max_lat: float):
    n = 2 ** z
    lon_span = (max_lon - min_lon) / n
    lat_span = (max_lat - min_lat) / n

    eps = 1e-12
    x0 = int(math.floor((bbox_min_lon - min_lon) / lon_span))
    x1 = int(math.floor((bbox_max_lon - min_lon - eps) / lon_span))

    # y is measured from the top (max_lat) downward.
    y0 = int(math.floor((max_lat - bbox_max_lat) / lat_span))
    y1 = int(math.floor((max_lat - bbox_min_lat - eps) / lat_span))

    x0 = max(0, min(n - 1, x0))
    x1 = max(0, min(n - 1, x1))
    y0 = max(0, min(n - 1, y0))
    y1 = max(0, min(n - 1, y1))

    if x1 < x0 or y1 < y0:
        return 0, -1, 0, -1
    return x0, x1, y0, y1


def main():
    parser = argparse.ArgumentParser(description="DEM pipeline: COGs -> EPSG:4326 tiles + tileset.json")
    parser.add_argument("--input", default="data/terrain/raw", help="Input directory with DEM COGs")
    parser.add_argument("--output", default="data/terrain", help="Output terrain directory")
    parser.add_argument(
        "--bbox",
        default=None,
        help="Optional bounds to tile: minLon,minLat,maxLon,maxLat (EPSG:4326 degrees). If omitted, tiles globally.",
    )
    parser.add_argument("--tile-size", type=int, default=256, help="Tile size in pixels")
    parser.add_argument("--zoom-min", type=int, default=0, help="Minimum zoom level")
    parser.add_argument("--zoom-max", type=int, default=2, help="Maximum zoom level")
    parser.add_argument("--sample-step", type=int, default=4, help="Vertex sampling step (for viewer)")
    parser.add_argument("--no-data", type=float, default=-9999.0, help="No-data value")
    args = parser.parse_args()

    require_gdal()

    input_dir = Path(args.input)
    output_dir = Path(args.output)
    tiles_dir = output_dir / "tiles"
    meta_dir = output_dir / "metadata"
    work_dir = output_dir / "working"

    output_dir.mkdir(parents=True, exist_ok=True)
    tiles_dir.mkdir(parents=True, exist_ok=True)
    meta_dir.mkdir(parents=True, exist_ok=True)
    work_dir.mkdir(parents=True, exist_ok=True)

    cogs = sorted(input_dir.glob("*.tif"))
    if not cogs:
        raise RuntimeError(f"No .tif files found in {input_dir}")

    vrt_path = work_dir / "dem_raw.vrt"
    run(["gdalbuildvrt", str(vrt_path)] + [str(p) for p in cogs])

    warped_path = work_dir / "dem_4326.tif"
    run(
        [
            "gdalwarp",
            "-t_srs",
            "EPSG:4326",
            "-r",
            "bilinear",
            "-dstnodata",
            str(args.no_data),
            "-overwrite",
            str(vrt_path),
            str(warped_path),
        ]
    )

    min_h, max_h = gdalinfo_stats(warped_path)

    if args.bbox:
        min_lon, min_lat, max_lon, max_lat = parse_bbox(args.bbox)
        min_lon = max(-180.0, min(180.0, min_lon))
        max_lon = max(-180.0, min(180.0, max_lon))
        min_lat = max(-90.0, min(90.0, min_lat))
        max_lat = max(-90.0, min(90.0, max_lat))
        if max_lon <= min_lon or max_lat <= min_lat:
            raise RuntimeError("bbox is empty after clamping")
    else:
        min_lon, min_lat, max_lon, max_lat = -180.0, -90.0, 180.0, 90.0

    for z in range(args.zoom_min, args.zoom_max + 1):
        n = 2 ** z
        if args.bbox:
            x0, x1, y0, y1 = tile_range_for_bbox(z, min_lon, min_lat, max_lon, max_lat, min_lon, max_lon, min_lat, max_lat)
            if x1 < x0 or y1 < y0:
                continue
            x_range = range(x0, x1 + 1)
            y_range = range(y0, y1 + 1)
        else:
            x_range = range(n)
            y_range = range(n)

        for y in y_range:
            for x in x_range:
                lon_min_t, lon_max_t, lat_min_t, lat_max_t = tile_bounds_in_bounds(z, x, y, min_lon, max_lon, min_lat, max_lat)
                out_dir = tiles_dir / str(z) / str(x)
                out_dir.mkdir(parents=True, exist_ok=True)
                out_path = out_dir / f"{y}.bin"
                if out_path.exists():
                    continue
                run(
                    [
                        "gdal_translate",
                        "-projwin",
                        str(lon_min_t),
                        str(lat_max_t),
                        str(lon_max_t),
                        str(lat_min_t),
                        "-projwin_srs",
                        "EPSG:4326",
                        "-outsize",
                        str(args.tile_size),
                        str(args.tile_size),
                        "-ot",
                        "Float32",
                        "-of",
                        "ENVI",
                        "-co",
                        "INTERLEAVE=BIL",
                        "-a_nodata",
                        str(args.no_data),
                        str(warped_path),
                        str(out_path),
                    ]
                )

    tileset = {
        "version": 1,
        "tile_size": args.tile_size,
        "zoom_min": args.zoom_min,
        "zoom_max": args.zoom_max,
        "data_type": "f32",
        "tile_path_template": "tiles/{z}/{x}/{y}.bin",
        "min_lon": min_lon,
        "max_lon": max_lon,
        "min_lat": min_lat,
        "max_lat": max_lat,
        "min_height": min_h,
        "max_height": max_h,
        "no_data": args.no_data,
        "sample_step": args.sample_step,
    }

    tileset_path = meta_dir / "tileset.json"
    tileset_path.write_text(json.dumps(tileset, indent=2))
    print(f"Wrote tileset: {tileset_path}")


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)
