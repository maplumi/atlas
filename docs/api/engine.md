# Engine API

## Engine lifecycle
- `Engine::new(desc)`
- `engine.update(dt, input)`
- `engine.render(frame)`

## Runtime budgeting

The runtime supports deterministic frame budgeting for time-slicing work.

Budgets are expressed in abstract "work units" rather than wall-clock time to keep scheduling replayable.

## Metrics

Runtime metrics are intended for observability only and must not affect semantic results.

The MVP metrics collector aggregates deterministically (stable key ordering; no wall-clock dependencies).

## Subsystems
- World/Scene
- Streaming
- Renderer
- Compute (programs + analysis)
- Metrics/Diagnostics
