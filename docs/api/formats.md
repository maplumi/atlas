# Formats API

Formats define how scenes and chunks are encoded/decoded.

## Current
- Scene manifest (`scene.manifest.json`) with immutable `package_id` and optional `content_hash`
- Vector chunk binary format (ATVC/AVc), including streamable `Read`/`Write` encode/decode and canonicalized JSON properties (stable key ordering)

## Planned
- terrain chunks
- analysis chunks
- streaming-friendly indexes
