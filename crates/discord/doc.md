# discord

Rich Presence: what Pulse puts on your Discord profile while a track plays.

## Files

- `lib.rs` — `Presence` (one snapshot of a playing track), `PresenceSettings`
  (what the user wants shown) and the client id lookup.
- `ipc.rs` — the worker thread and the activity builder.
- `art.rs` — the album-art cache and the iTunes lookup behind it.
- `build.rs` — reads `DISCORD_CLIENT_ID` from the environment or `.env` and
  bakes it in.

## The shape

`Presence` is a whole snapshot, not a diff: title, artist, album, duration, the
wall-clock second playback started, whether it is paused, and the settings. The
app sends one every time something meaningful changes; the worker decides
whether that is worth an IPC round trip.

`PresenceSettings` is shared between the settings file and the activity builder
on purpose. It lives in this crate with `serde` derives, and the app stores it
directly, so a field added to one side is a compile error on the other. That is
the guard against a switch in Settings that does nothing.

## Non-obvious behaviour

- **No client id, no presence.** `client_id()` reads `DISCORD_CLIENT_ID` from
  the environment or `.env` at build time and bakes it in with
  `cargo:rustc-env`. `is_available()` is false without it, and the app shows a
  card explaining that instead of switches that would be inert.
- **Cover art is a URL, so it cannot be local.** Discord renders `large_image`
  as a key or a URL; there is no upload path from a desktop app. The lookup goes
  through `cover_search`, one request per artist+album, and the answer is cached
  in `discord_art_cache.json` in the config directory — including negative
  answers, so a missing cover costs one request ever.
- **A cover has to be the right album.** Taking the top search result put other
  people's records on the profile: the first hit for an album title is often a
  compilation, a deluxe edition or a different artist. Candidates are filtered
  through `cover_search::matching`, the same comparison the library's own cover
  fetch uses, and only a match is accepted. iTunes is asked first, MusicBrainz
  second, and a miss is cached only when both answered — a network failure is
  retried rather than remembered.
- **The cache is versioned.** Entries written by a build that took the first
  result were not verified, so `FORMAT_VERSION` discards them instead of
  trusting them.
- **A paused timer would lie.** Timestamps are dropped while paused rather than
  frozen at the pause point, because Discord counts up from `start` on its own
  clock. The small image/text flips to "Paused" instead.
- **`keep_on_pause` is the app's decision, not this crate's.** The bridge either
  clears the activity or re-sends the snapshot with `paused: true`.
- **Updates are rate limited.** `MIN_INTERVAL` is five seconds and `dirty`
  collapses bursts: the app can send a snapshot on every position tick without
  the worker flooding the socket. Reconnects back off for fifteen seconds.
- **Seek detection is the app's job.** The bridge re-sends when the engine's
  position drifts more than 2.5 s from the anchor it is counting from, which is
  what makes a seek show up promptly without polling.
- **One thread, one connection.** `DiscordHandle` is a channel handle; dropping
  it does not close the socket, `shutdown()` does, and the app calls it on quit
  so the activity does not linger after the process exits.
