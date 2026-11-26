# Changelog

Newest first. This file is the single source for release notes: the release
workflow publishes the matching section as the GitHub release body, and Pulse
shows the same lines in Settings → Updates before it installs anything.

Write for someone deciding whether to update: what changed, why it matters, and
anything they have to do. Installation instructions do not belong here — the
workflow appends them after the `---`.

## 5.9.1 — 2026-11-26

### Added

- **Release notes before an update installs.** "Check for updates" now stops at
  what is available and shows what changed; the download starts only when you
  press Install.
- Release bodies are generated from this file, so the GitHub release page and
  the app cannot disagree about what changed.

## 5.9.0 — 2026-11-26

### Changed

- **The AppImage is the install, and it is portable.** Settings, the library
  database, artwork and logs live in `data/` beside the image instead of being
  scattered into `~/.config/pulse-music` and `~/.local/share/pulse-music`.
  Deleting `~/Applications/Pulse` removes Pulse and everything it wrote.
- The Arch package installs to `~/Applications/Pulse/Pulse.AppImage` and keeps
  the launcher and menu entry, so the menu and the folder agree on one file.
- The in-app updater replaces the image inside that folder, which is user-owned
  and needs no root.

### Fixed

- An AppImage run from a read-only place — `/opt`, a mounted ISO — falls back to
  the platform layout instead of silently failing to save settings.

### Upgrading

- First launch copies the library and settings from the old platform folders,
  and leaves the originals in place. Delete them once the library looks right.
- Existing flat installs (`~/Applications/Pulse.AppImage`) are still launched by
  the package's launcher, so nothing has to move by hand.
