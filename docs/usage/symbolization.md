# Symbolization

Symbolization controls color/size/visibility and is intended to be programmable.

## MVP
- Declarative layer style (visibility/color/lift)

### Web viewer API (MVP)
The web viewer exposes a minimal symbology API over WASM for layer styling:

- `set_layer_visible(id, visible)`
- `set_layer_color_hex(id, "#rrggbb")`
- `set_layer_opacity(id, opacity)`
- `set_layer_lift(id, lift)`
- `get_layer_style(id)` → `{ visible, color_hex, opacity, lift }`

Additional size controls used by the viewer:

- `set_city_marker_size(px)` / `get_city_marker_size()`
- `set_line_width_px(px)` / `get_line_width_px()`

The web UI wires these controls for built-in layers and uploaded datasets.

## Planned
- Deterministic sandboxed programs for cartography and behavior
