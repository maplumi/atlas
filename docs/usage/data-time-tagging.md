# Temporal Tagging Rules

Every dataset must be time-tagged either:
- Explicitly (user-provided dataset validity window)
- Implicitly (from feature attributes like timestamp columns)

## Supported temporal models
- Instant events: `t`
- Validity intervals: `[t_start, t_end]`
- Mixed: per-feature time + dataset window

## Default behavior
If time is missing:
- Dataset is treated as always-active (explicitly noted in metadata)
