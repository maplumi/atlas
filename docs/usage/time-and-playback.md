# Time and Playback

Time is first-class. All queries and rendering are evaluated over a time window.

## Time Dial UI

A scrollable timeline floats at the top-center of the map. It shows:

- **Tick marks** with major/minor divisions at a configurable interval
- **Center needle** indicating the current engine time
- **Playback controls** (step back, play/pause, step forward)
- **Speed indicator** displaying the current playback multiplier

### Interaction

| Action | Effect |
|---|---|
| **Drag** the track left/right | Scrub through time |
| **Scroll wheel** over the track | Step forward/backward by one tick |
| **Play/Pause** button (or `Space`) | Toggle auto-playback |
| **Step** buttons | Jump by one tick interval |
| **T** key | Toggle dial visibility |

### Playback modes

- **Loop** – wraps around from end → start
- **Play once** – stops at the end
- **Bounce** – reverses direction at boundaries (ping-pong)

### Settings

Click the ⚙ gear icon on the right end of the dial to open the Time Settings modal:

- **Time mode**: relative (seconds) or absolute (date/time)
- **Playback speed**: 0.25× to 60×
- **Start / End**: define the time range
- **Step size**: fixed timestep per frame (default ~16ms)
- **Loop mode**: loop, play once, bounce
- **Time format**: seconds, HH:MM:SS, or date/time
- **Tick interval**: spacing of tick marks in seconds
- **Show/hide**: toggle dial visibility

Settings are persisted in the browser (IndexedDB with localStorage fallback) and survive page reloads.

## Layer Time Configuration

Each layer can opt-in to time-based filtering via the Time Settings modal.
Enable time on a layer and specify which attribute field (e.g. `timestamp`,
`date`, `event_time`) drives its temporal visibility. When time-awareness is
enabled, features outside the current time window are hidden.

### Supported temporal models
- Instant events: `t`
- Validity intervals: `[t_start, t_end]`
- Mixed: per-feature time + dataset window

### Default behavior
If time is missing from a dataset, it is treated as always-active.
