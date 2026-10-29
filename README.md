<div align="center">

# 🎧 Pulse

**A personal, local-first music player for Linux.**

One library. No account. No cloud.

</div>

---

## What it does

**Library** — indexes the folders you choose and keeps watching them. Browse by
album, artist, folder, playlist or one flat All tracks list; `Ctrl+K` searches
everything; shift- and ctrl-click select rows across a list. Cue sheets are
understood, and a re-rip or a moved folder doesn't lose your play history.

**Playback** — native sample rate through PipeWire, or an exclusive grab if you
prefer. ReplayGain (per track or per album, with clipping protection) read from
the tags your files already carry. A real play-next queue, shuffle and repeat,
and a full-screen cover view.

**Editing** — a tag editor for one track or a whole album, including cover art
and raw tag rows, followed by a rescan of exactly what changed. Lyrics from
embedded tags, `.lrc` sidecars or LRCLIB, with a button to save what you fetched.

**Housekeeping** — a duplicate finder that keeps one copy of each song by folder
priority and trashes the rest, daily rotated database backups with one-click
restore, an integrity report, and a portable export that carries its own data.

**Phone remote** — an optional web UI, embedded in the binary and installable
to a home screen: albums, artists, playlists, folders and the whole catalog,
with search, a queue you can reorder or swipe away, and lyrics that follow
along.

## Offline by default

Nothing touches the network unless you ask. Everything online lives on one
**Settings → Online** page and starts off:

| | Default |
|---|---|
| Lyrics from the internet (LRCLIB) | off |
| Discord Rich Presence | off |
| Remote control web server | off |

No accounts, no analytics, no telemetry, no scrobbling.

## Requirements

- Linux with **PipeWire** and `pipewire-alsa`
- `alsa-lib`, `libxkbcommon`, `libxcb` at runtime
- A Rust toolchain to build from source

## Install

Prebuilt, from [Releases](../../releases):

```sh
tar -xzf pulse-*-x86_64-linux.tar.gz
install -m 755 pulse ~/.local/bin/pulse
```

From source:

```sh
cargo build --release -p pulse
install -m 755 target/release/pulse ~/.local/bin/pulse
```

The remote UI in `web/` is compiled into the binary, so build it before a
release build or the binary ships without it (a debug build never embeds it):

```sh
cd web && npm install && npm run build
```

## Keyboard

| Key | Action |
|---|---|
| `Ctrl+K` | command palette |
| `Space` | play / pause |
| `←` `→` | seek |
| `Ctrl+←` `Ctrl+→` | previous / next track |
| `↑` `↓` | volume |
| `Q` | queue the hovered track to play next |
| `Esc` | leave the cover view |

## Where things live

| | Path |
|---|---|
| Settings | `~/.config/pulse-music/settings.json` |
| Library database | `~/.local/share/pulse-music/library.db` |
| Backups | `~/.local/share/pulse-music/backups/` |
| Logs | `~/.local/share/pulse-music/logs/` |
| Covers and artwork | `~/.cache/pulse-music/` |

The directory is `pulse-music`, never `~/.config/pulse` — that one belongs to
PipeWire-Pulse. Set **`PULSE_HOME`** to move all of it into one directory, which
is what makes an exported copy independent of the machine it came from.

## Development

```sh
make generate              # audio fixtures, once (needs ffmpeg)
cargo test --workspace     # ~1,000 tests
cargo clippy --workspace --all-targets
```

```
crates/
  pulse/            the application (produces the `pulse` binary)
  pulse_remote/     remote control server + embedded web UI
  music_library/    SQLite schema, queries, identity and adoption
  music_indexer/    scanning, tag parsing, cue sheets
  tag_writer/       metadata writing
  audio_*/          decoder, engine, PipeWire/ALSA output
  lyrics/           local and LRCLIB lyrics
  discord/          Rich Presence
web/                the remote UI (Svelte)
```

Every crate has a `doc.md` beside its source: what it does, why, and which parts
look wrong but are load-bearing.

## License

MIT — see [`LICENSE`](LICENSE).

Pulse is built on [Pawse](https://github.com/popovpsk/pawse) by Aleksandr Popov.
