# Performance Tuning

## Principles
- Prefer baked chunk metadata for pruning.
- Prefer indices over brute-force scans.
- Keep hot-path data contiguous.

## Practical knobs (current)
- Chunk size and feature density
- Quantization settings in chunk formats
