# Scene Package (SCN)

A scene package is an immutable, versioned container with:
- Manifest (`scene.manifest.json`) including `content_hash` and immutable `package_id`
- Content-addressed blobs
- Spatial/temporal indexes

Goal: offline reproducibility + streamability.

## Identity

When present:
- `content_hash` is a deterministic hash of the manifest contents.
- `package_id` must equal `content_hash`.

## Tooling

The `crates/tools` binary can assemble a package directory:
- `atlas manifest <output_dir> <chunk.avc> [chunk2.avc ...] [--name NAME]`
