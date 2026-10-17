# music_library

The SQLite-backed library: the catalog the screens browse, the user data that
hangs off it (playlists, fetched lyrics, listening history), and the identity
layer that keeps the two apart. Only this crate
touches the database; `pawse::library_service` drives scans through
[`ScanWrite`] and everything else goes through [`LibraryRepository`].

## Files

- `lib.rs` — crate root, re-exports, and the bulk of the tests (GUI-less,
  against a temp database). The migration tests build a real schema-v8 database
  by replaying the migration steps through version 8 and seeding it, then open it
  through `SqliteLibrary::open_at`.
- `repository.rs` — the `LibraryRepository` and `ScanWrite` traits.
- `sqlite.rs` — the only implementation: `SqliteLibrary` (one connection),
  `ScanSession` (the batched scan writer on its own connection),
  the migration runner, and the SQL shared between them (`CLEAR_CATALOG`,
  `RETIRE_UNSEEN_LOCAL_BINDINGS`, `SWEEP_UNREFERENCED_ITEMS`,
  `REFRESH_ITEM_SNAPSHOTS`).
- `migrations.rs` — `MIGRATIONS`, the versioned schema steps.
- `genres.rs` — `normalize_genres`, the one rule that turns raw genre values into
  catalog genres (split on `,` `;` `/`, collapse whitespace, drop junk, dedup
  case-insensitively). It lives here rather than in `music_indexer`, its only
  caller, because the normalizing rule is part of the row shape this crate owns.
- `models.rs` — row and transfer types (`Track`, `ScanTrack`, `LocalFolder`, …).
  Code holding a `Track` asks the track itself instead of parsing its path:
  `Track::local_file` and `own_file` (a local file that is this track's alone —
  not a cue piece — i.e. the one whose tags and sidecar lyrics belong to it).
  `local_file` also rejects the legacy `pawse-source://` prefix that a pre-3.0
  build wrote for a media-server track, so such a row reports "no local file"
  instead of being handed to the tag editor or the decoder; the prefix lives in
  the private `REMOTE_LOCATOR_PREFIX`. Parsing a bare locator string is left to
  the layers that only have strings (playback's `playback_locators` walk).
- `adoption.rs` — the pure matching that decides which files are the same track
  (`match_tracks`), plus `normalize_tag`, the one tag normalization the three
  tiers share.
- `album_artists.rs` — deriving an album's credited artists from its tracks.
- `thumbnail.rs` — cover thumbnail generation.
- `error.rs` — `LibraryError`.

## Two kinds of rows

**Catalog** — `tracks`, `albums`, `artists`, `genres`, `track_artists`,
`track_album_artists`, `track_genres`, `album_artists`, `cover_art`, and
disk-derived lyrics (`lrc` / `embedded`). Everything here is re-derivable from
the files. A scan is still `clear()` + refill for this part, and that is fine:
`clear()` touches nothing else.

**Identity and user data** — `media_items`, `media_bindings`, `sources`,
`playlist_tracks`, `lyrics` from the network, `plays` and `play_deliveries`.
None of it is derivable, so no scan deletes it. `loves` and `love_deliveries`
are still in the schema but nothing writes them any more — see "Legacy rows
kept in the schema".

The link between them is the id. `media_items.id` is the durable identity of a
track; `tracks.id` borrows it (`tracks.id REFERENCES media_items(id)`), so a
rescan that deletes and refills `tracks` puts every row back under the id it
had. Every user table references `media_items`, never `tracks`. The column is
still called `track_id` in those tables — the values are identical and
renaming would have been churn across every query for no behavioural gain.

## Identity: sources, bindings, items

- `sources` — one row per library root. Every row anything writes is
  `kind = 'local'`, one per configured music folder (`uri` = the folder path);
  the column and the protocol-specific kinds it used to hold are still in the
  table, but nothing creates them now. Source `1` is a disabled placeholder that
  migration 9 hangs every pre-existing binding on, because the migration cannot
  see `settings.json`.
- `media_bindings` — "this item is available at this place": `(source_id,
  source_key, start_offset_ms)` is unique, `source_key` is the absolute file
  path. `present` and `last_seen_scan` record whether the last scan saw it.
- `media_items` — the identity, plus a display snapshot (`title`, `artist`,
  `album`, `duration_ms`, `cover_art_id`) used to render an item that has no
  `tracks` row any more.

`reconcile_local_sources(folders)` makes `sources` match the configured folders:
it disables every local source, then upserts one enabled row per folder with its
`available` flag. A folder removed from settings therefore becomes a *disabled*
source whose bindings stay put — re-adding the same folder re-enables it and
every binding matches again by path, so playlist entries come back under their
old ids.

`ScanSession::insert_track` looks a scanned file up by
`(path, start_offset_ms)` in the session's binding map, which covers **all**
local sources (placeholder and disabled ones included), re-points the row to the
longest matching enabled root, and only mints a new item when nothing matches.
That is what makes the placeholder self-healing: the first real scan after
migration moves every binding onto the right folder without a separate step.
That scan must actually happen: `has_unplaced_media()` reports present bindings
still on the placeholder, and `library_service` skips the fast path while it is
true. So `finish` retires (`present = 0`) every placeholder binding that is under
no enabled root, or under an available one (which this scan walked completely
and did not find it in). Only bindings under an offline folder stay unplaced. A
file deleted before the upgrade would otherwise keep the fast path off forever.
Retiring loses nothing: the placeholder is disabled, so `present` there affects
neither playback nor matching, and a file that comes back is still found by its
key and moved onto its folder.

## Offline folders

`library_service` probes every configured folder before a scan (`ScanScope`): a
folder that cannot be listed, or is empty while bindings under it are still
present (an unmounted mount point), is *unavailable*. The scan goes ahead over
the rest. The unavailable folder's source gets `available = 0`, its bindings are
not retired and its items are not swept — offline is not the same as deleted.
They stay candidates for matching, though: the same file showing up somewhere
else (the folder renamed on disk and added again, a backup copy) joins them. Its
tracks drop out of the catalog (the scan did not re-add them), so they disappear
from the library screens and show as unavailable in playlists until the folder
is back; then the next scan finds every binding by path and they return under
the same ids.

With every folder offline the scan still runs and the catalog ends up empty:
offline means hidden, whether one folder is gone or all of them. Playlists keep
their entries, shown as unavailable.

The fast-path key (`scan_meta.folders`) marks offline folders with a `?` prefix,
so going offline and coming back each force one real scan. With every folder
online the key is the same plain sorted list it always was.

## Identity across sources

**The rule.** An item has at most one live binding per source. Two files in one
source are two items (a copy next to its original stays a separate track); the
same file under two folders is one item with one binding in each. Which binding
plays and names the catalog row is priority, not identity: the folder with the
lower source id, resolved in `ScanSession::place`, and walked the same way by
`playback_locators`.

`playback_locators(item)` lists every place an item can play from right now —
present bindings on enabled, available sources, by source id then binding id.
The catalog row only names one of them and is refreshed by the next scan, so
when a file has vanished since, playback tries the next entry instead of
failing. Cue pieces of one image come back as several locators that share a path
and differ by offset, which is what cue playback needs.

**What a match needs.** Only what is in the database, so a folder that is
offline, disabled or gone still takes part:

- `media_bindings.file_size` (from the indexer's stat) with the cue offset — the
  same bytes, even untagged;
- the `media_items` snapshot (first artist, title, album, duration), which
  `refresh_item_snapshots` keeps current while an item is seen and which
  freezes when it is not.

Relative paths are not used: anything a relative path would find between two
folders the size finds too, and a cue piece shares its path with its siblings.

`adoption::match_tracks` runs three tiers, strongest first, each over every
unmatched track before the next starts:

1. `file` — a shared `(size, cue offset)`, duration within 2 s when both are
   known.
2. `tags` — normalized first artist (or an alias) + title + album, duration
   within 5 s when both are known. The same size wins among several, then the
   closest duration, then the lowest id.
3. `title` — normalized artist + title, duration within 5 s and required, and
   only for a candidate with no present binding at all (truly dead). An offline
   item is never merged with another recording that merely shares a title.

A candidate that already has a live binding in the arriving track's source is
skipped. Within a tier every passing pair is ranked first (same file, then
closest duration, then lowest id) and assigned best-first, so the order tracks
arrive in never decides who gets a candidate; a claimed candidate leaves the
pool. Tag tiers need a non-empty artist, so untagged `01.flac`s only ever match
by size.

**When matching runs — two events.**

- *Birth*: a file the source has no binding for. Arrivals are held until
  `finish` (the candidates are only known once the scan has seen everything; on
  an empty database with one folder there is nothing to match and they are
  inserted directly) and settled source by source in id order, so a second new
  folder in the same scan sees the items the first one just created. Candidates
  are all items, live, dead or offline. A match only inserts a binding and an
  `adoptions` row.
- *Death*: after local bindings are retired, `revive_lost_items` takes the items
  that carry user data and have no playable binding left (file deleted, folder
  offline or removed) and matches them against the playable items that carry
  none. A match *absorbs* the spare item: its `tracks` row, catalog links, disk
  lyrics and bindings move to the lost item's id (foreign keys deferred for the
  move) and the empty item is deleted. Nothing user-side is merged — the
  absorbed item had none. This covers a copy made before the original went away,
  and a copy that only existed as a separate item because its tags differed.

Everything is loaded once per pass (`load_identities`: items, bindings with
their source state, and the set with user data) and matched in memory, so a pass
costs two queries whatever the library size.

Two items that are both alive and both missed each other (older data, a tag
edit that made them equal later) are never merged automatically; that is for a
manual screen.

**One row per item.** A second binding of an item already written this scan
(the same file in another folder) does not write another row — unless its source
has priority, and then the row is rewritten from that file entirely (tags, year,
cover, `is_cue`, disk lyrics), so the row never depends on which file the
parallel indexer delivered first. A second binding in a source the item was
already placed in this scan (`placed`: an adopted item whose old file came back
from the trash) gets its old binding dropped and a fresh item, so neither file
disappears from the library. The same `(path, start_offset_ms)` delivered twice
in one scan is not such a case: nested folders in the settings walk the file
twice, and two cue sheets for one image yield the same tracks twice. `seen`
drops the repeat before anything else, or it would split off a fresh item every
scan and leave the catalog row on an item without a binding.

A held arrival that fails to write is logged and skipped, as `add_track` errors
always were; it must not fail `finish`, or retire, sweep and the fingerprint
would never run again for as long as that file is there.

The scan writer opens its batches with `BEGIN IMMEDIATE`. The matching passes
start a batch with reads; in WAL mode a deferred transaction that reads, sees
the UI commit a playlist edit, and then writes fails with `SQLITE_BUSY_SNAPSHOT`,
which the busy timeout does not retry. Taking the write lock up front makes the
UI wait instead. A batch opens only when there is something to write (a held
arrival writes nothing) and closes after 256 writes or 250 ms, and
`library_service` calls `flush` whenever it has caught up with the indexer (its
channel is empty). So the lock is not held while the indexer parses the next
files, and other writers — a playlist edit — get in between batches. `finish` is
one transaction: absorbing an item moves rows under deferred foreign keys, which
only works inside one. The price is that `finish` holds the lock for as long as
writing every held arrival takes — about 65 ms for 2000 new files, seconds for a
first scan of tens of thousands, during which a playlist edit waits on the busy
timeout.

The UI-side writers that read before they write (`add_tracks_to_playlist` —
`add_track_to_playlist` is its one-track form —, `remove_track_from_playlist`,
`move_track_in_playlist`) open their transaction with
`TransactionBehavior::Immediate` for the same reason: a deferred transaction
that already read is refused `SQLITE_BUSY` at once while a scan batch holds the
lock, without the busy timeout ever waiting, and the edit was silently lost.

`add_tracks_to_playlist` appends the whole batch in one transaction, in the given
order, skipping tracks already in the playlist (and repeats within the batch) via
the `(playlist_id, track_id)` unique index; ids with no `media_items` row (a queue
can still hold the playing track after a rescan dropped it, or a queue restored from
settings) are skipped too, so one stale entry cannot fail the whole batch on the
foreign key. Positions advance only for rows that were actually inserted, so they
stay contiguous.

## Legacy rows kept in the schema

3.0 removed the media-server layer and the Liked feature from the API. Their
storage was *not* dropped, deliberately, and a reader who "cleans up" the
leftovers will break an upgrade:

- `remote_tracks` is created by an early migration and removed in 4.0. A
  database written by an older build still carries it through the earlier steps,
  which is why migration 11 still reads it: it drops the scan fingerprint when a
  cached `remote_tracks.genre` would come out different from `normalize_genres`
  (a separator, stray or repeated whitespace, a junk value (`Unknown`, `Other`,
  …) or a bare number). On a fresh install that step matches no rows.
- `loves`, `love_deliveries`, `liked_track_ids` and `remote_tracks` are all
  **dropped by migration 12**, together with the whole Liked idea. The
  `media_items_guard_user_data` trigger is dropped and recreated in the same step
  without its `loves` clause, because SQLite lets you drop a table a trigger
  names and leaves behind a trigger that fails only when it fires.
- `seed_liked_playlist_from_column` (migration 9, in `sqlite.rs`) still runs. It
  folds the legacy `tracks.liked` column into a real playlist before migration 9's
  table rebuild drops the column, so an old database cannot lose its likes on
  upgrade. It returns early when no row has `liked = 1`, so a fresh install never
  grows an empty playlist it cannot use.
  Migration 12 then **retires** the result: an empty seeded playlist is deleted
  outright, and one that actually holds tracks is renamed to "Imported likes" and
  left alone as an ordinary playlist. Deleting it would destroy a curated list of
  somebody's — the change is meant to remove a *feature*, not a person's data.
- The legacy `pawse-source://<source_id>/<key>.<suffix>` locator prefix stays
  too. `Track::local_file` / `own_file` reject it privately
  (`REMOTE_LOCATOR_PREFIX` in `models.rs`), so a server-track row from an old
  database reports "no local file" rather than being handed to the tag editor or
  the decoder. The crate and package names (`pawse`, `pawse_remote`,
  `crates/pawse`) are unchanged as well.

## What a scan does to identity

`ScanSession::finish` (only after the indexer reported `Complete` — see below):

0. Settles held arrivals (adoption, above).
1. `RETIRE_UNSEEN_LOCAL_BINDINGS` — bindings of enabled, available local
   sources that this scan did not see get `present = 0`. Disabled and offline
   sources are left alone.
2. `SWEEP_UNREFERENCED_ITEMS` — deletes items that have no `tracks` row, no
   present binding on an enabled source, and nothing user-side pointing at them.
   This bounds growth: removing a 50k-track folder leaves only the items somebody
   filed in a playlist, listened to, fetched lyrics for, or loved before 3.0.

`library_service::run_scan` drops the session without calling `finish` when the
indexer stopped before `Complete` (a panicked worker closes the channel). A
partial enumeration must never retire bindings or record a fingerprint.

## Stable album and artist ids

The catalog is cleared and refilled by every scan, and after a `DELETE` SQLite
would hand out ids from 1 again in whatever order the parallel indexer delivered
files — every scan reshuffled them, and a screen still holding an old id (an
open artist page, the web remote) linked to a different album. So
`ScanSession::clear` snapshots the ids first (`StableIds`): an album gets its
old id back by `(title, year)` — the same key `resolve_album` merges on — and
an artist by name. A new one gets an id above everything ever handed out
(`allocate_catalog_id`); the high-water marks live in `scan_meta`
(`album_id_high`, `artist_id_high`), so a removed album's id is not given to
the next new one. The snapshot raises the marks to its own maximum before the
clear, and every writer allocates through the same function — the scan and the
tag editor's `upsert_album` / `get_or_insert_artist` — so a tag edit that lands
between two scan batches can neither reuse a dead id nor take one the scan is
about to hand back. Genres are not covered: their ids are
reissued on every scan, so the one screen that holds a genre (the app's genre
page) holds its `key` — the Rust-lowercased name, unique and stable — and every
genre query takes the key.

Artists are keyed by the lowercased name (`artist_key`): `Glass the Harbor`
and `Glass The Harbor`, or `NOVA` and `Nova`, are one artist, shown under the
name met first. Tags differ in case across sources and rips far more often than
two real artists do. Lowercasing is Rust's (Unicode), not SQLite's `NOCASE`
(ASCII only), so the tag editor's `get_or_insert_artist` compares in Rust too.
Albums still merge on the exact title.

## Invariants and what enforces them

- **Nothing user-side is lost to a scan.** `clear()` never touches user tables,
  and the `media_items_guard_user_data` trigger makes it structural: deleting an
  item referenced by `playlist_tracks`, network lyrics, `plays` or `loves`
  aborts. `SWEEP_UNREFERENCED_ITEMS` mirrors the trigger's conditions exactly;
  if the two ever diverge the sweep aborts the scan's last transaction instead of
  deleting user data.
- **Ids are stable across rescans and never reused.** `tracks.id` is borrowed,
  not minted, and `media_items.id` is `AUTOINCREMENT`: once the sweep deletes an
  item its number is retired. Without that SQLite hands out `max(rowid) + 1`, so
  sweeping the newest item would give its id to the next new track, and anything
  still holding the old id (the web remote before its refetch, a persisted queue)
  would show a different song.
- **A pre-3.0 love still guards its item.** Nothing writes `loves` any more, but
  the trigger and the sweep both name it, so an item a like once pointed at
  survives a scan like any other piece of user data. See "Legacy rows kept in
  the schema".
- **Lyrics keep one row per source** (`PRIMARY KEY (track_id, source)`, migration
  10): a `.lrc` sidecar, the embedded tag and an LRCLIB result live side by side, so
  a scan never overwrites fetched words and the user can switch between them.
  `clear()` removes the disk rows (`lrc` / `embedded`) so a deleted `.lrc`
  disappears; fetched rows (and LRCLIB `not_found` markers) survive.
  `lyrics_variants` returns every row; which one is shown is decided outside SQL by
  `lyrics_source::choices` / `pick`: default order `lrc > embedded > lrclib`, or
  `lrclib > lrc > embedded` when the user prefers LRCLIB; rows whose text equals a
  higher-ranked one are dropped, so an exported `.lrc` hides the LRCLIB copy it came
  from. Migration 10 drops and recreates `media_items_guard_user_data`, because
  `ALTER TABLE … RENAME` refuses while a trigger names the dropped `lyrics`.

## Unavailable tracks

`track_artists_map` falls back to the item snapshot's artist for ids that have
no `track_artists` rows (i.e. unavailable ones), so a greyed playlist row still
shows who it is. It binds every id through one JSON parameter (`json_each`)
rather than one placeholder per id: callers pass the whole library, and SQLite
caps bound parameters at 32766.

`tracks_for_playlist` returns every entry, joined through `media_items`: an
entry without a `tracks` row comes back with `available = false`, its
title/duration/cover from the item snapshot and its path from a binding.
Returning every entry is deliberate — positions in `playlist_tracks` must line
up with what the UI shows, or `move_track_in_playlist` would move the wrong row.
Catalog reads (`all_tracks`, albums, artists) only ever see `tracks`, so
unavailable items never appear there.

`refresh_item_snapshots` copies the current catalog values into the snapshot. It
runs in `settle_derived_rows` **before** the orphan sweep: the sweep keeps any
cover a snapshot still references, so refreshing afterwards would keep a replaced
cover alive forever.

## Reading listening history

`plays` is written only through `record_play` and read through three queries on
the main connection:

- `play_tallies(since)` — plays grouped by artist / title / album,
  case-insensitively, with total and qualified counts and the last play. Counts
  every row, qualified or not.
- `recent_plays(limit)` — raw rows, newest first.
- `track_listings()` — one row per catalog track with its first artist, album,
  album artist, year and play stats joined on `plays.track_id` (the item id).
  Plays of tracks that left the catalog are only visible through
  `play_tallies`.

The Tools screen that consumed these is gone, so nothing in the app calls them
today; they stay on the trait and are exercised by the crate's tests.

## Migrations

There are users: schema changes are versioned steps in `MIGRATIONS`, never a
wipe. `run_migrations`:

- backs the database up before crossing `IDENTITY_MIGRATION` (9), once:
  `VACUUM INTO <db>.bak-v<old>.partial`, then an atomic rename to
  `<db>.bak-v<old>`. An existing `.bak-v<old>` is kept; a leftover `.partial`
  (crash mid-backup) is never trusted and gets rewritten;
- turns `foreign_keys` **off** for the migration transaction. This is required,
  not a shortcut: migration 9 rebuilds tables (`CREATE x_new`, copy, `DROP x`,
  `RENAME`), and with foreign keys on, `DROP TABLE tracks` would cascade into
  every child that references `tracks` by name — including the freshly copied
  new tables. The pragma cannot change inside a transaction, so it is set before
  `BEGIN` and restored after;
- counts `pragma_foreign_key_check` rows before and after the steps and rolls
  back only if the migration **added** violations. Pre-existing junk in someone's
  catalog (a dangling `track_artists` row) must not turn every launch into a
  failed open.

Everything this release adds to the schema — identity, sources, bindings,
adoptions, and the `remote_tracks` cache that is now only history — is one step,
migration 9: a user upgrades once, from 8. A development database that ran the
earlier split steps (up to 12) has the same schema; set its `user_version` back
to 9, or it would skip the next step.

Migration 9 keeps ids one-to-one (`media_items.id := tracks.id`), so user rows
are copied as-is and can be verified by counts and sums. Child tables that
reference a rebuilt parent only by name (`track_artists`, `play_deliveries`, …)
are not rebuilt: after the rename they resolve to the new parent. Before the SQL
runs, `seed_liked_playlist_from_column` folds a legacy `tracks.liked` column into
a real playlist — that rebuild is what drops the column, so the seeding has to
happen first. It does nothing for a database with no liked rows (see "Legacy
rows kept in the schema").
