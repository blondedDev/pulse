# media_integration

Bridges the player to the Linux desktop's "now playing" surface — the MPRIS
interface, which is what GNOME/KDE shells, media keys and most tray widgets read.
It is a thin adapter: it owns no playback state and makes no policy decisions.

Two directions cross this boundary:

- **Outbound**: the app pushes `NowPlayingInfo`, `MediaPlaybackState`, and elapsed
  position to MPRIS.
- **Inbound**: the desktop sends transport commands (play / pause / next / seek …)
  back as `MediaCommand` values down a `flume::Sender` the caller supplied. The
  receiving end lives in `pawse::media_bridge`, not here.

## Responsibilities

- Define the platform-agnostic surface: the `SystemMediaIntegration` trait, the
  `NowPlayingInfo` / `MediaPlaybackState` / `MediaCommand` data types, and the
  `create_integration(sender)` factory.
- Provide the `souvlaki`-backed MPRIS implementation behind that trait.
- Degrade to a no-op when there is no session bus or initialization fails.

## Files

- `lib.rs` — the trait, the data types, and `create_integration` (returns `None`
  on init failure). Forbids `unsafe` crate-wide.
- `souvlaki_backend.rs` — `SouvlakiIntegration` over `souvlaki` with the pure-Rust
  `zbus` backend, so the build needs no system `libdbus` headers. `map_event` maps
  a `MediaControlEvent` to a `MediaCommand`; `playback` maps state + elapsed into a
  `souvlaki::MediaPlayback`. Unit tests cover both.

## Non-obvious behavior

- **Main thread only.** `souvlaki::MediaControls` is `!Send`/`!Sync`. The
  integration is handed out as `Rc<dyn SystemMediaIntegration>` and never crosses
  threads. Only the `flume::Sender` (which is `Send`) leaves the thread, captured by
  souvlaki's attach handler.

- **Commands go through a channel, never a direct call.** The attach handler can
  fire at any time. It only does a non-blocking `send(MediaCommand)`; the actual
  engine/queue mutation happens later in the caller's GPUI loop. This keeps the
  D-Bus callback off GPUI's runtime and its internal locks.

- **Two update paths, deliberately asymmetric.** `update_now_playing` rebuilds the
  full metadata set once per track. `update_position` is the hot path (the engine
  emits position at ~5 Hz) and only patches elapsed time + playback rate instead of
  rebuilding it.

- **Playback rate is what freezes the scrubber.** MPRIS clients advance from the
  reported rate, not from the playback-state flag. So both `update_now_playing` and
  `update_position` derive the rate from `MediaPlaybackState` (`1.0` playing, `0.0`
  otherwise); a paused track must report rate `0.0` or the shell keeps visually
  scrubbing. This is why `update_now_playing` takes the state rather than assuming
  the track is playing.

- **Progress is sanitized before it reaches the bus.** `souvlaki_backend::playback`
  carries a value only when it `is_finite()` and non-negative —
  `Duration::from_secs_f64` panics on `NaN` / `inf` / overflow, and the `f64` fields
  of the public `NowPlayingInfo` are not otherwise validated.

- **Graceful degradation.** `create_integration` returns `None` when souvlaki fails
  to create or attach its controls (typically: no session bus). The app then runs
  without any system media surface, and `MediaBridge` subscribes to a no-op instead.

- **The artwork file must live somewhere the shell can read.** `NowPlayingInfo`
  carries a filesystem path, handed to MPRIS as `mpris:artUrl = file://…`, which the
  desktop shell resolves **outside** our process. `LibraryService::get_cover_art_path_for_media`
  therefore writes into `dirs::cache_dir()/pulse-music/artwork`, not
  `std::env::temp_dir()`.

## Lifetime

The integration is owned by `MainView` and dies with the window, which is correct
because the app quits when the last window closes. This wiring lives in
`pawse::media_bridge`; see `.docs/project.md`.

## Scope

Pulse 3.0 is Linux-only. The macOS `MediaPlayer`/AppKit backend and the Windows
SMTC backend were deleted along with the rest of the non-Linux platform support, and
with them the `hwnd` plumbing in `create_integration` and `PlatformConfig`.
