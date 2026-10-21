# pulse_remote

The phone/desktop remote: an Axum HTTP server plus a WebSocket state stream,
serving the Svelte app in `web/` and a small JSON API. GPUI-free — the app
drives it through `StateHandle` (state out) and `CommandSink` (commands in), so
it never touches the queue or the engine directly.

Off by default (`remote_enabled` in settings.json). When disabled the server is
not started at all and `PlayerState::idle()` is published so a stale client
shows nothing rather than a frozen track.

## Files

- `lib.rs` — the protocol: `PlayerState`, `QueueItem`, the library DTOs,
  `Command`, `LibraryReader`, and `spawn` (the socket, thread and shutdown
  handle).
- `http.rs` — the Axum router and handlers.
- `assets.rs` — `rust-embed` of `../../web/dist`. **Release builds only**: the
  router's static fallback is behind `#[cfg(not(debug_assertions))]`, so a debug
  build serves the API but not the UI.
- `build.rs` — warns during a release build when `web/dist/index.html` is
  missing, and registers the dist tree for rebuilds.

## Two directions

- **State out.** The app calls `StateHandle::publish` with a whole `PlayerState`
  or `publish_position` for the hot path. A `watch` channel fans it out to every
  WebSocket and `/api/state`. `is_serving()` (receiver count > 1) lets the app
  skip building a state nobody is listening for.
- **Commands in.** Clients `POST /api/command` with a `Command`; it lands on a
  flume channel the app drains on its own loop, so nothing here runs engine or
  queue code on an HTTP thread.

`publish_position` merges into the existing snapshot instead of replacing it, so
the ~5 Hz position tick does not rebuild the queue array.

## Endpoints

| Route | Returns |
|---|---|
| `GET /api/state` | the current `PlayerState` |
| `GET /api/queue` | the queue items (also streamed once per `queue_rev` change) |
| `GET /api/cover?id=&size=small\|large\|full` | cover bytes |
| `GET /api/artists` | artist list with up to four cover ids |
| `GET /api/artist?id=&full=` | one artist's albums and tracks |
| `GET /api/playlists` | playlist summaries |
| `GET /api/playlist?id=` | one playlist's tracks |
| `GET /api/folders?parent=` | one folder-tree level: the roots when `parent` is empty |
| `GET /api/folder?path=` | every track at or below `path` |
| `GET /api/tracks` | the whole catalog in display order |
| `POST /api/command` | a `Command`, 202 on acceptance |
| `GET /ws` | WebSocket, one `PlayerState` per change |

`LibraryReader` is the narrow interface those read endpoints go through: the app
implements it with `LibraryAccess`, which holds the repository handle and the
artist grouping, so the server never sees `LibraryService`.

## Non-obvious behaviour

- **`PROTOCOL_VERSION` is in every state payload.** Bump it when a field's
  meaning changes; the web app ships inside the binary, so a mismatch normally
  means a stale page, not a server from another version.
- **The folder endpoints are tree-shaped, not flat.** `folders(parent)` exists so
  the phone can walk the same hierarchy the desktop's Folders tab shows, and
  `/api/folder` answers with the whole subtree in the library's display order.
  `FolderEntry.track_count` is that subtree count, so the UI can offer "all
  tracks in this folder" without downloading them first.
- **`/api/tracks` is the entire catalog.** It is a few hundred kilobytes on a
  large library; the web app renders it in pages of 200 rather than all at once.
- **No authentication.** The server binds `0.0.0.0` on the configured port and
  trusts the network it is on. That is the documented model; do not expose the
  port beyond a LAN you control.
- **Shutdown is a oneshot + join.** `RemoteServer::drop` sends the shutdown
  signal and joins the thread, so toggling the setting off actually closes the
  listener instead of leaving it bound.
