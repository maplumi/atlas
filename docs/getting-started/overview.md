# Overview

Atlas is not a mapping library. It is a spatiotemporal computation engine whose outputs include visualization.

## What Atlas does
- Ingests datasets (static or streaming) and compiles them into native chunk formats
- Maintains spatial + temporal indices for fast query and retrieval
- Renders layers in 2D and 3D using WebGPU
- Runs analysis natively and can visualize analysis outputs immediately

## Key properties
- Deterministic by default
- Precision-safe global rendering (CPU f64, GPU camera-relative f32)
- Time is first-class (features and datasets can be time-bounded)
- Programmable symbolization and behavior via sandboxed programs
