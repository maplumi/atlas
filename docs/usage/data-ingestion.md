# Data Ingestion

Atlas accepts common formats for ingestion (GeoJSON, MVT, glTF, XYZ raster), but compiles them into native chunk formats for speed and determinism.

## Ingestion pipeline
1. Validate input schema
2. Attach/derive temporal metadata
3. Reproject/normalize CRS
4. Compile into chunk formats (VCH/TCH/...)
5. Package into a scene package (SCN)

## Determinism requirement
Ingestion output must be stable for the same input + options.
