<div align="center">

# 🎧 Pulse

**A personal, local-first music player for Linux.**

One library. One listener. No account, no cloud.

</div>

---

Pulse runs in a single native window, indexes the music you already own, and
tries hard to stay out of the way while it plays.

## ✨ What it does

### 📚 Library
- 🗂️ Indexes folders you choose — and keeps watching them, so new files appear
  without a click.
- 🎼 Browse by **album**, **artist**, **genre**, **folder**, **playlist**, or one
  flat **All Tracks** list.
- ⚡ **Ctrl+K** opens a command palette over everything.
- 💿 Cue-sheet aware, with an adoption layer so a re-rip or a moved folder
  doesn't duplicate your play history.

### ▶️ Playback
- 🔊 Native sample rate through PipeWire, exclusively if you want it.
- ⏭️ A real **play-next queue** — queue a song and it plays next; queue more and
  they follow in the order you queued them.
- 🔀 Shuffle and repeat, an always-there **cover view**, and a now-playing bar
  that tells the truth.

### ✏️ Editing
- 🏷️ **Tag editor** for one track or a whole album, with cover art and raw tag
  rows, followed by a rescan of exactly what changed.
- 📝 Lyrics from embedded tags, `.lrc` sidecars, or LRCLIB on request — and one
  button to save what you fetched as a `.lrc` beside the track.

### 🧭 Extras
- 📊 **Rewind** — listening totals and your top artists, albums and tracks.
- 🛟 **Data safety** — daily rotated database backups, one-click restore, and an
  integrity report that counts problems without deleting anything.
- 📦 **Portable export** — copy the program, optionally with your settings and
  library, into a folder that carries its own data.

## 🔌 Offline by default

Nothing touches the network unless you ask. Everything online lives on one
**Settings → Online** page and starts off:

| Feature | Default |
|---|---|
| 🌐 Lyrics from the internet (LRCLIB) | off |
| 🟣 Discord Rich Presence | off |
| 📱 Remote control web server | off |

No accounts, no analytics, no crash reporting, no scrobbling.

## 📋 Requirements

- 🐧 Linux with **PipeWire** and `pipewire-alsa`
- 🦀 A Rust toolchain (to build from source)
- 📚 `alsa-lib`, `libxkbcommon`, `libxcb` at runtime

## 📥 Install

### Prebuilt binary

Download the tarball from [Releases](../../releases), then:

```sh
tar -xzf pulse-*-x86_64-linux.tar.gz
install -m 755 pulse ~/.local/bin/pulse
```

### From source

```sh
cargo build --release -p pawse
install -m 755 target/release/pulse ~/.local/bin/pulse
```

The crate is `pawse`; the binary it produces is `pulse`. That split is
deliberate and is not going to be "fixed".

The remote UI in `web/` is a Svelte app embedded into the binary at build time —
rebuild it after changing anything there:

```sh
cd web && npm install && npm run build
```

## ⌨️ Keyboard

| Key | Action |
|---|---|
| `Ctrl+K` | command palette |
| `Space` | play / pause |
| `←` `→` | seek |
| `Ctrl+←` `Ctrl+→` | previous / next track |
| `↑` `↓` | volume |
| `Q` | queue the hovered track to play next |
| `Esc` | leave the cover view |

## 📂 Where things live

| What | Path |
|---|---|
| ⚙️ Settings | `~/.config/pulse-music/settings.json` |
| 💾 Library database | `~/.local/share/pulse-music/library.db` |
| 🛟 Backups | `~/.local/share/pulse-music/backups/` |
| 📜 Logs | `~/.local/share/pulse-music/logs/` |
| 🖼️ Cache (covers, artwork) | `~/.cache/pulse-music/` |

The directory is `pulse-music` — never `~/.config/pulse`, which belongs to
PipeWire-Pulse.

Set **`PULSE_HOME`** to point all of the above at one directory. That is what
makes an exported copy independent of the machine it came from.

## 🛠️ Development

```sh
cargo test --workspace     # ~955 tests
cargo clippy --workspace --all-targets
```

A release build takes a few minutes; a no-op incremental build is under a
second.

```
crates/
  pawse/            the application (binary name: pulse)
  pawse_remote/     remote control server + embedded web UI
  music_library/    SQLite schema, queries, identity/adoption
  music_indexer/    scanning, tag parsing, cue sheets
  audio_*/          playback engine, decoders, PipeWire/ALSA output
  lyrics/           local and LRCLIB lyrics
  tag_writer/       metadata writing
  discord/          Rich Presence
web/                the remote UI (Svelte)
```

Each crate has a `doc.md` next to its source with the design notes: what it
does, why it does it that way, and the parts that look wrong but are
load-bearing.

## 📝 Notes

- Crate and package names are still `pawse` / `pawse_remote`; only the product,
  the binary and the on-disk paths were renamed.
- Track rows written by an older build may carry a `pawse-source://` locator. It
  is recognised and treated as "no local file" rather than handed to the
  decoder.
- Motion is optional by design: every animation has its own switch plus a master
  switch. Turned off, the app is exactly as usable — just still.

## 📄 License

MIT — see [`LICENSE`](LICENSE).

Pulse is built on [Pawse](https://github.com/popovpsk/pawse) by Aleksandr Popov.
