use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rusqlite::{Connection, OptionalExtension};

use crate::adoption::{Descriptor, Tier, WHOLE_FILE, match_tracks};
use crate::album_artists::{AlbumTrackArtists, derive_album_artists};
use crate::backup;
use crate::error::{LibraryError, Result};
use crate::migrations::MIGRATIONS;
use crate::models::{
    AlbumSearchEntry, AlbumSummary, ArtistGrouping, ArtistSummary, CoverArt, FolderSummary,
    IntegrityReport, LocalFolder, NewPlay, NewTrack, PlayTally,
    PlaylistSummary, RecentPlay, ScanTrack, SourceSummary, StoredLyrics, Track,
    TrackListing, ListeningStats, StatEntry,
};
use crate::repository::{LibraryRepository, ScanWrite};

/// Tracks committed per transaction during a batched scan. One `fsync` per
/// batch (with `synchronous = NORMAL`) instead of one per track.
const SCAN_BATCH_SIZE: usize = 256;
const SCAN_BATCH_TIME: std::time::Duration = std::time::Duration::from_millis(250);

const TRACK_COLUMNS: &str = "id, path, title, album_id, track_number, disc_number, \
    duration_ms, year, cover_art_id, start_offset_ms, bitrate, is_cue, 1";

const TRACK_COLUMNS_T: &str = "t.id, t.path, t.title, t.album_id, t.track_number, \
    t.disc_number, t.duration_ms, t.year, t.cover_art_id, t.start_offset_ms, t.bitrate, \
    t.is_cue, 1";

const PLAYLIST_ENTRY_COLUMNS: &str = "m.id, COALESCE(t.path, b.source_key, ''), \
    COALESCE(t.title, m.title), t.album_id, t.track_number, COALESCE(t.disc_number, 1), \
    COALESCE(t.duration_ms, m.duration_ms), t.year, COALESCE(t.cover_art_id, m.cover_art_id), \
    COALESCE(t.start_offset_ms, b.start_offset_ms, 0), t.bitrate, COALESCE(t.is_cue, 0), \
    t.id IS NOT NULL";

const PLACEHOLDER_SOURCE_ID: i64 = 1;

/// The tail every album-grouped display list sorts by, after the album title.
///
/// `al.id` is not decoration. Albums merge on the **exact** title
/// (`upsert_album` matches `title = ?1`), but these lists sort the title
/// `COLLATE NOCASE`, so "Twin" and "twin" are two rows that compare equal here.
/// Without the id between them their tracks interleave by (disc, track, title),
/// and `pawse::library_views::grouped_tracks_view` — which groups *consecutive*
/// album ids into one album block — renders one album as several identical
/// headers with its songs split across them. That is a real library, not a
/// contrived one: a re-rip or a different tagger casing the title is enough.
///
/// `t.id` does the same job one level down. Two files in one root are two
/// tracks by design, so an album can legitimately hold the same (disc, track)
/// twice, and without a final tiebreaker their order is whatever SQLite feels
/// like on the day.
const ALBUM_RUN_TIEBREAK: &str = "al.id, t.disc_number, t.track_number, t.title, t.id";

/// Ranked lists for the listening summary. Each selects
/// `name, subtitle, plays, seconds`, which is what [`ranked`] maps.
///
/// Grouping is `COLLATE NOCASE` so "Tool" and "tool" are one entry; ordering
/// then breaks ties by name to keep the list stable between openings.
const RANKED_ARTISTS: &str = "SELECT artist, NULL, COUNT(*), COALESCE(SUM(played_secs), 0) \
     FROM plays WHERE started_at >= ?1 AND artist <> '' \
     GROUP BY artist COLLATE NOCASE \
     ORDER BY COUNT(*) DESC, artist COLLATE NOCASE LIMIT ?2";

const RANKED_ALBUMS: &str = "SELECT album, artist, COUNT(*), COALESCE(SUM(played_secs), 0) \
     FROM plays WHERE started_at >= ?1 AND album IS NOT NULL AND album <> '' \
     GROUP BY album COLLATE NOCASE, artist COLLATE NOCASE \
     ORDER BY COUNT(*) DESC, album COLLATE NOCASE LIMIT ?2";

const RANKED_TRACKS: &str = "SELECT title, artist, COUNT(*), COALESCE(SUM(played_secs), 0) \
     FROM plays WHERE started_at >= ?1 AND title <> '' \
     GROUP BY title COLLATE NOCASE, artist COLLATE NOCASE \
     ORDER BY COUNT(*) DESC, title COLLATE NOCASE LIMIT ?2";

fn map_track_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        album_id: row.get(3)?,
        track_number: row.get(4)?,
        disc_number: row.get(5)?,
        duration_ms: row.get(6)?,
        year: row.get(7)?,
        cover_art_id: row.get(8)?,
        start_offset_ms: row.get(9)?,
        bitrate: row.get(10)?,
        is_cue: row.get::<_, i64>(11)? != 0,
        available: row.get::<_, i64>(12)? != 0,
    })
}

/// Run one of the ranked-list queries. Every one of them selects
/// `name, subtitle, plays, seconds` in that order, so they share a mapper.
fn ranked(conn: &Connection, sql: &str, since: i64, limit: i64) -> Result<Vec<StatEntry>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(rusqlite::params![since, limit], |row| {
        Ok(StatEntry {
            name: row.get(0)?,
            subtitle: row.get::<_, Option<String>>(1)?.filter(|s| !s.is_empty()),
            plays: row.get::<_, i64>(2)?.max(0) as u64,
            seconds: row.get::<_, i64>(3)?.max(0) as u64,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::Database)
}

fn display_ordered_tracks(conn: &Connection, where_clause: &str) -> Result<Vec<Track>> {
    let sql = format!(
        "SELECT {TRACK_COLUMNS_T} FROM tracks t \
         LEFT JOIN albums al ON al.id = t.album_id \
         LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.position = 0 \
         LEFT JOIN artists art ON art.id = aa.artist_id \
         {where_clause} \
         ORDER BY art.sort_name COLLATE NOCASE, COALESCE(al.year, 0), \
         al.title COLLATE NOCASE, {ALBUM_RUN_TIEBREAK}",
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], map_track_row)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::Database)
}

fn artist_membership_sql(grouping: ArtistGrouping) -> &'static str {
    match grouping {
        ArtistGrouping::TrackArtist => "SELECT artist_id, track_id, position FROM track_artists",
        ArtistGrouping::AlbumArtist => {
            "SELECT artist_id, track_id, position FROM track_album_artists \
             UNION ALL \
             SELECT aa.artist_id, t.id, aa.position FROM tracks t \
             JOIN albums al ON al.id = t.album_id AND al.artist_known = 1 \
             JOIN album_artists aa ON aa.album_id = al.id \
             WHERE NOT EXISTS (SELECT 1 FROM track_album_artists x WHERE x.track_id = t.id) \
             UNION ALL \
             SELECT ta.artist_id, ta.track_id, ta.position FROM track_artists ta \
             JOIN tracks t ON t.id = ta.track_id \
             LEFT JOIN albums al ON al.id = t.album_id \
             WHERE NOT EXISTS (SELECT 1 FROM track_album_artists x WHERE x.track_id = ta.track_id) \
             AND COALESCE(al.artist_known, 0) = 0"
        }
    }
}

fn membership_ctes(grouping: ArtistGrouping) -> String {
    let listed = artist_membership_sql(grouping);
    let credited = match grouping {
        ArtistGrouping::TrackArtist => "SELECT * FROM m".to_string(),
        ArtistGrouping::AlbumArtist => {
            "SELECT * FROM m UNION SELECT artist_id, track_id, position FROM track_artists"
                .to_string()
        }
    };
    format!("m AS ({listed}), u AS ({credited})")
}

fn map_artist_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtistSummary> {
    Ok(ArtistSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        sort_name: row.get(2)?,
        track_count: row.get(3)?,
    })
}

fn no_metadata_artist(track_count: i64) -> ArtistSummary {
    ArtistSummary {
        id: crate::NO_METADATA_ARTIST_ID,
        name: String::new(),
        sort_name: String::new(),
        track_count,
    }
}

fn orphan_track_count(conn: &Connection, grouping: ArtistGrouping) -> Result<i64> {
    let membership = artist_membership_sql(grouping);
    let sql = format!(
        "WITH m AS ({membership}) SELECT COUNT(*) FROM tracks t \
         WHERE NOT EXISTS (SELECT 1 FROM m WHERE m.track_id = t.id)"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let count: i64 = stmt.query_row([], |row| row.get(0))?;
    Ok(count)
}

pub struct SqliteLibrary {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

/// Remove the SQLite database and its WAL sidecar files. Used when an
/// incompatible on-disk schema is detected (no users → no migrations).
fn remove_db_files(db_path: &Path) {
    let _ = std::fs::remove_file(db_path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = db_path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
}

fn premigration_schema(db_path: &Path) -> bool {
    let Ok(conn) = Connection::open(db_path) else {
        return false;
    };
    let Ok(version) = conn.query_row("SELECT user_version FROM pragma_user_version", [], |row| {
        row.get::<_, i64>(0)
    }) else {
        return false;
    };
    version == 0
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // WAL lets the scan-writer connection commit concurrently with UI reads on
    // the main connection; NORMAL drops the per-commit fsync to one per WAL
    // checkpoint — the single biggest reindex speedup, especially on Windows.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -16384)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // The scan writer holds a transaction open across each batch; without a
    // busy timeout a concurrent UI write (e.g. liking a track mid-scan) would
    // fail with SQLITE_BUSY instead of waiting for the batch to commit.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn compress_lyrics(text: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(text.as_bytes());
    encoder.finish().unwrap_or_default()
}

fn decompress_lyrics(blob: &[u8]) -> Option<String> {
    let mut out = String::new();
    match ZlibDecoder::new(blob).read_to_string(&mut out) {
        Ok(_) => Some(out),
        Err(e) => {
            log::warn!(
                "Failed to decompress lyrics blob ({} bytes): {e}",
                blob.len()
            );
            None
        }
    }
}

const IDENTITY_MIGRATION: i32 = 9;

/// Pulse 4.0 removed the Liked idea. This is the step where what is left of it
/// leaves the schema, and where the playlist the v9 migration seeded from the
/// legacy `liked` column is retired.
const LIKED_REMOVAL_MIGRATION: i32 = 12;

const ALBUM_ID_HIGH: &str = "album_id_high";
const ARTIST_ID_HIGH: &str = "artist_id_high";

struct StableIds {
    albums: HashMap<(String, Option<i32>), i64>,
    artists: HashMap<String, i64>,
}

impl StableIds {
    fn load(conn: &Connection) -> Result<Self> {
        let albums: HashMap<(String, Option<i32>), i64> = {
            let mut stmt = conn.prepare("SELECT id, title, year FROM albums")?;
            stmt.query_map([], |row| Ok(((row.get(1)?, row.get(2)?), row.get(0)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        let artists: HashMap<String, i64> = {
            let mut stmt = conn.prepare("SELECT id, name FROM artists ORDER BY id")?;
            let rows: Vec<(String, i64)> = stmt
                .query_map([], |row| Ok((row.get(1)?, row.get(0)?)))?
                .collect::<std::result::Result<_, _>>()?;
            let mut artists = HashMap::new();
            for (name, id) in rows {
                artists.entry(artist_key(&name)).or_insert(id);
            }
            artists
        };
        raise_id_high(conn, ALBUM_ID_HIGH, albums.values().copied().max())?;
        raise_id_high(conn, ARTIST_ID_HIGH, artists.values().copied().max())?;
        Ok(Self { albums, artists })
    }
}

fn artist_key(name: &str) -> String {
    name.to_lowercase()
}

fn id_high(conn: &Connection, key: &str) -> Result<i64> {
    Ok(conn
        .query_row("SELECT value FROM scan_meta WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0))
}

fn raise_id_high(conn: &Connection, key: &str, seen: Option<i64>) -> Result<()> {
    let Some(seen) = seen else {
        return Ok(());
    };
    if seen > id_high(conn, key)? {
        conn.execute(
            "INSERT INTO scan_meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, seen.to_string()],
        )?;
    }
    Ok(())
}

fn allocate_catalog_id(conn: &Connection, table: &str, high_key: &str) -> Result<i64> {
    let max: i64 = conn.query_row(
        &format!("SELECT COALESCE(MAX(id), 0) FROM {table}"),
        [],
        |row| row.get(0),
    )?;
    let id = id_high(conn, high_key)?.max(max) + 1;
    raise_id_high(conn, high_key, Some(id))?;
    Ok(id)
}

const CLEAR_CATALOG: &str = "DELETE FROM track_artists; \
    DELETE FROM track_album_artists; \
    DELETE FROM track_genres; \
    DELETE FROM album_artists; \
    DELETE FROM tracks; \
    DELETE FROM albums; \
    DELETE FROM artists; \
    DELETE FROM genres; \
    DELETE FROM lyrics WHERE source IN ('lrc', 'embedded');";

const RETIRE_UNSEEN_LOCAL_BINDINGS: &str = "UPDATE media_bindings SET present = 0 \
    WHERE present = 1 AND last_seen_scan <> ?1 \
    AND source_id IN (SELECT id FROM sources WHERE kind = 'local' AND enabled = 1 AND available = 1)";

const IDENTITY_ITEMS: &str =
    "SELECT id, title, artist, album, duration_ms FROM media_items ORDER BY id";

const IDENTITY_BINDINGS: &str = "SELECT b.item_id, b.source_id, s.kind = 'local', \
    s.enabled, s.available, b.present, b.last_seen_scan, b.file_size, b.start_offset_ms, \
    b.start_offset_ms > 0 OR EXISTS (SELECT 1 FROM media_bindings o \
        WHERE o.source_key = b.source_key AND o.start_offset_ms > 0 AND o.source_id = b.source_id) \
    FROM media_bindings b JOIN sources s ON s.id = b.source_id";

const ITEMS_WITH_USER_DATA: &str = "SELECT track_id FROM playlist_tracks \
    UNION SELECT track_id FROM lyrics WHERE source NOT IN ('lrc', 'embedded') \
    UNION SELECT track_id FROM plays WHERE track_id IS NOT NULL";

const UNWRITE_TRACK: [&str; 5] = [
    "DELETE FROM track_artists WHERE track_id = ?1",
    "DELETE FROM track_album_artists WHERE track_id = ?1",
    "DELETE FROM track_genres WHERE track_id = ?1",
    "DELETE FROM lyrics WHERE track_id = ?1 AND source IN ('lrc', 'embedded')",
    "DELETE FROM tracks WHERE id = ?1",
];

const ABSORB_ITEM: [&str; 9] = [
    "UPDATE tracks SET id = ?1 WHERE id = ?2",
    "UPDATE track_artists SET track_id = ?1 WHERE track_id = ?2",
    "UPDATE track_album_artists SET track_id = ?1 WHERE track_id = ?2",
    "UPDATE track_genres SET track_id = ?1 WHERE track_id = ?2",
    "INSERT INTO lyrics (track_id, source, text, not_found, updated_at) \
        SELECT ?1, source, text, not_found, updated_at FROM lyrics WHERE track_id = ?2 \
        ON CONFLICT(track_id, source) DO UPDATE SET text = excluded.text, \
        not_found = excluded.not_found, updated_at = excluded.updated_at",
    "DELETE FROM lyrics WHERE track_id = ?2",
    "UPDATE media_bindings SET item_id = ?1 WHERE item_id = ?2",
    "DELETE FROM adoptions WHERE item_id = ?2",
    "DELETE FROM media_items WHERE id = ?2",
];

const SWEEP_UNREFERENCED_ITEMS: &str = "DELETE FROM media_items WHERE \
    NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.id = media_items.id) \
    AND NOT EXISTS (SELECT 1 FROM media_bindings b JOIN sources s ON s.id = b.source_id \
        WHERE b.item_id = media_items.id AND b.present = 1 AND s.enabled = 1) \
    AND NOT EXISTS (SELECT 1 FROM playlist_tracks WHERE track_id = media_items.id) \
    AND NOT EXISTS (SELECT 1 FROM lyrics WHERE track_id = media_items.id \
        AND source NOT IN ('lrc', 'embedded')) \
    AND NOT EXISTS (SELECT 1 FROM plays WHERE track_id = media_items.id)";

const REFRESH_ITEM_SNAPSHOTS: &str = "UPDATE media_items SET \
    title = c.title, artist = c.artist, album = c.album, duration_ms = c.duration_ms, \
    cover_art_id = c.cover_art_id, updated_at = ?1 \
    FROM ( \
        SELECT t.id AS id, t.title AS title, \
            COALESCE((SELECT a.name FROM track_artists ta JOIN artists a ON a.id = ta.artist_id \
                WHERE ta.track_id = t.id ORDER BY ta.position LIMIT 1), '') AS artist, \
            (SELECT al.title FROM albums al WHERE al.id = t.album_id) AS album, \
            t.duration_ms AS duration_ms, t.cover_art_id AS cover_art_id \
        FROM tracks t \
    ) c \
    WHERE c.id = media_items.id \
    AND (media_items.title IS NOT c.title OR media_items.artist IS NOT c.artist \
        OR media_items.album IS NOT c.album OR media_items.duration_ms IS NOT c.duration_ms \
        OR media_items.cover_art_id IS NOT c.cover_art_id)";

fn sidecar(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = db_path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn backup_before_migration(conn: &Connection, db_path: &Path, version: i32) -> Result<()> {
    let target = sidecar(db_path, &format!(".bak-v{version}"));
    if target.exists() {
        return Ok(());
    }
    let partial = sidecar(db_path, &format!(".bak-v{version}.partial"));
    let _ = std::fs::remove_file(&partial);
    conn.execute("VACUUM INTO ?1", [partial.to_string_lossy().into_owned()])?;
    std::fs::rename(&partial, &target)?;
    Ok(())
}

/// Delete backups beyond the daily window plus one per month, and sweep any
/// partial copy left behind by a run that died mid-write.
///
/// Best effort deliberately: failing to delete an old backup is not a reason to
/// fail a launch, so errors are logged and swallowed.
fn prune_backups(dir: &Path, stem: &str, retain: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();

    // A partial copy from today may be in flight, so only older ones are junk.
    let today = backup::backup_file_name(stem, unix_now());
    for name in &names {
        let Some(owner) = partial_owner(name) else {
            continue;
        };
        if owner != today && backup::is_backup_name(stem, owner) {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }

    for name in backup::backups_to_delete(stem, &names, retain) {
        if let Err(err) = std::fs::remove_file(dir.join(&name)) {
            log::warn!("library: could not prune old backup {name}: {err}");
        }
    }
}

/// The backup name a partial copy belongs to, if the file is one.
///
/// `VACUUM INTO` writes its target as an ordinary database, so a copy that is
/// interrupted can leave a rollback journal beside it (`<name>.partial-journal`)
/// as well as the partial itself. All of those belong to the same backup and are
/// swept together — otherwise they would sit in the directory forever, since
/// nothing else in this crate looks at them.
fn partial_owner(name: &str) -> Option<&str> {
    for suffix in [".partial-journal", ".partial-wal", ".partial-shm", ".partial"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return Some(base);
        }
    }
    None
}

/// Remove a partial copy and any SQLite sidecar that may have been made beside
/// it, before starting a new copy under the same name.
fn remove_partial_copy(dir: &Path, partial: &str) {
    let _ = std::fs::remove_file(dir.join(partial));
    for suffix in ["-journal", "-wal", "-shm"] {
        let _ = std::fs::remove_file(dir.join(format!("{partial}{suffix}")));
    }
}

/// The marker that says "replace the database with this file on next launch".
fn restore_marker(db_path: &Path) -> PathBuf {
    sidecar(db_path, ".restore-pending")
}

/// Apply a restore that was requested from the UI, if there is one.
///
/// This runs before the database is opened, which is the only moment the file can
/// be replaced safely. The database being replaced is kept beside the new one, so
/// a restore is itself undoable — the opposite of what the word usually means,
/// and the reason it is safe to put a button for it in a settings page.
fn apply_pending_restore(db_path: &Path) -> Result<Option<PathBuf>> {
    let marker = restore_marker(db_path);
    if !marker.exists() {
        return Ok(None);
    }
    let requested = std::fs::read_to_string(&marker)
        .ok()
        .map(|raw| PathBuf::from(raw.trim().to_owned()));
    // The marker goes regardless of what happens next: one that kept failing
    // would try to replace the library again on every single launch.
    let _ = std::fs::remove_file(&marker);
    let Some(backup) = requested else {
        log::warn!("library: restore marker was unreadable; keeping the current library");
        return Ok(None);
    };
    if !backup.exists() {
        log::warn!(
            "library: a restore was requested from {} but that backup is gone; keeping the current library",
            backup.display()
        );
        return Ok(None);
    }
    let aside = sidecar(db_path, &format!(".replaced-{}", unix_now()));
    if db_path.exists() {
        std::fs::rename(db_path, &aside)?;
    }
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sidecar(db_path, suffix));
    }
    std::fs::copy(&backup, db_path)?;
    log::warn!(
        "library: restored {}; the database it replaced is at {}",
        backup.display(),
        aside.display()
    );
    Ok(Some(aside))
}

fn foreign_key_violations(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?,
    )
}

/// Migration-only: fold the legacy `tracks.liked` column into a real playlist
/// before the identity migration drops the column.
///
/// This is history, not a live feature. Pulse 3.0 removed the Liked tab, but the
/// migration must keep doing what it always did: an old database that still has
/// the column must not lose those likes on upgrade. Nothing else calls this, and
/// the playlist it creates is an ordinary user playlist afterwards.
fn seed_liked_playlist_from_column(conn: &Connection) -> Result<()> {
    if stored_liked_playlist(conn)?.is_some() {
        return Ok(());
    }
    // Only a database that actually carried likes gets the playlist. A fresh
    // install runs this migration too, and must not come up with an empty
    // "Liked" playlist it can never use.
    let liked: i64 = conn.query_row("SELECT COUNT(*) FROM tracks WHERE liked = 1", [], |row| {
        row.get(0)
    })?;
    if liked == 0 {
        return Ok(());
    }
    let playlist_id = create_liked_playlist(conn)?;
    conn.execute(
        "INSERT INTO playlist_tracks (playlist_id, position, track_id) \
         SELECT ?1, ROW_NUMBER() OVER (ORDER BY art.sort_name COLLATE NOCASE, \
             COALESCE(al.year, 0), al.title COLLATE NOCASE, t.disc_number, \
             t.track_number, t.title) - 1, t.id \
         FROM tracks t \
         LEFT JOIN albums al ON al.id = t.album_id \
         LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.position = 0 \
         LEFT JOIN artists art ON art.id = aa.artist_id \
         WHERE t.liked = 1",
        [playlist_id],
    )?;
    Ok(())
}

fn stored_liked_playlist(conn: &Connection) -> Result<Option<i64>> {
    let stored = conn
        .query_row(
            "SELECT value FROM scan_meta WHERE key = 'liked_playlist_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse::<i64>().ok());
    let Some(id) = stored else {
        return Ok(None);
    };
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM playlists WHERE id = ?1)",
        [id],
        |row| row.get(0),
    )?;
    Ok(exists.then_some(id))
}

fn create_liked_playlist(conn: &Connection) -> Result<i64> {
    conn.execute(
        "INSERT INTO playlists (name, created_at) VALUES ('Liked', ?1)",
        [unix_now()],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO scan_meta (key, value) VALUES ('liked_playlist_id', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [id.to_string()],
    )?;
    Ok(id)
}

/// Pulse 4.0 migration step: retire the playlist the v9 migration seeded from
/// the legacy `tracks.liked` column.
///
/// The feature is gone, so a playlist called "Liked" must not survive as
/// something that looks like one. The two cases are genuinely different:
///
/// * **Empty** — it is the artefact the user is complaining about, and it holds
///   nothing, so it is deleted.
/// * **Non-empty** — it holds a real, curated list of tracks somebody marked.
///   Deleting that would be destroying user data to tidy up a name, so it is
///   renamed instead and left as an ordinary playlist.
fn retire_liked_playlist(conn: &Connection) -> Result<()> {
    // `None` means nothing was ever seeded, or the marker points at a playlist
    // that is already gone — either way there is nothing to retire. The marker is
    // cleared regardless, so a later upgrade cannot resurrect the name.
    if let Some(id) = stored_liked_playlist(conn)? {
        let tracks: i64 = conn.query_row(
            "SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1",
            [id],
            |row| row.get(0),
        )?;
        if tracks == 0 {
            conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        } else {
            log::warn!(
                "library: keeping the migrated playlist {id} (it holds {tracks} tracks) \
                 under the name 'Imported likes'"
            );
            conn.execute(
                "UPDATE playlists SET name = 'Imported likes' WHERE id = ?1",
                [id],
            )?;
        }
    }
    conn.execute("DELETE FROM scan_meta WHERE key = 'liked_playlist_id'", [])?;
    Ok(())
}

fn apply_migrations(conn: &mut Connection, from: i32) -> Result<()> {
    let tx = conn.transaction()?;
    let violations_before = foreign_key_violations(&tx)?;
    for (version, sql) in MIGRATIONS.iter() {
        if from < *version {
            if *version == IDENTITY_MIGRATION {
                seed_liked_playlist_from_column(&tx)?;
            }
            if *version == LIKED_REMOVAL_MIGRATION {
                retire_liked_playlist(&tx)?;
            }
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", *version)?;
        }
    }
    let violations_after = foreign_key_violations(&tx)?;
    if violations_after > violations_before {
        return Err(LibraryError::InvalidData(format!(
            "migration raised foreign key violations from {violations_before} to {violations_after}"
        )));
    }
    tx.commit()?;
    Ok(())
}

struct LocalRoot {
    path: PathBuf,
    source_id: i64,
    available: bool,
}

fn load_local_roots(conn: &Connection) -> Result<Vec<LocalRoot>> {
    let mut stmt = conn
        .prepare("SELECT id, uri, available FROM sources WHERE kind = 'local' AND enabled = 1")?;
    let mut roots = stmt
        .query_map([], |row| {
            Ok(LocalRoot {
                source_id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                available: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    roots.sort_by_key(|root| std::cmp::Reverse(root.path.components().count()));
    Ok(roots)
}

fn root_for_path<'a>(roots: &'a [LocalRoot], path: &str) -> Option<&'a LocalRoot> {
    let path = Path::new(path);
    roots.iter().find(|root| path.starts_with(&root.path))
}

fn source_for_path(roots: &[LocalRoot], path: &str) -> i64 {
    root_for_path(roots, path).map_or(PLACEHOLDER_SOURCE_ID, |root| root.source_id)
}

/// Normalises a directory path for folder comparisons: no trailing separator,
/// with the filesystem root left intact.
fn clean_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches(std::path::MAIN_SEPARATOR);
    // `D:\` trims down to a bare drive designator, which is a drive-relative
    // path rather than a directory. Its separator is part of the root, so
    // `D:Rock` never comes out of a join.
    if trimmed.is_empty() || trimmed.ends_with(':') {
        format!("{trimmed}{}", std::path::MAIN_SEPARATOR)
    } else {
        trimmed.to_string()
    }
}

/// Builds a `LIKE` pattern matching everything strictly below `dir`.
///
/// The trailing separator is what keeps `.../Rock` from also matching a
/// sibling like `.../Rocknroll`, and the `ESCAPE` guards directories whose
/// names contain a real `%` or `_`. `\` is escaped because it is the escape
/// character itself.
fn folder_like_pattern(dir: &str) -> String {
    folder_like_pattern_with(dir, std::path::MAIN_SEPARATOR)
}

/// `separator` is injected so both platform shapes are testable from one host:
/// stored track paths are host-native, so a pattern ending in a hardcoded `/`
/// silently matched nothing at all on Windows.
fn folder_like_pattern_with(dir: &str, separator: char) -> String {
    let mut pattern = String::with_capacity(dir.len() + 2);
    for ch in dir.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    // A filesystem root already *is* the separator (`/`, `D:\`). Appending
    // another would demand a path with an empty leading component, so a library
    // root of `/` matched nothing at all.
    if !dir.ends_with(separator) {
        // The separator needs escaping when it is the escape character itself,
        // which on Windows it is: a bare `\` followed by the wildcard would read
        // as an escaped `%` and demand a literal percent sign.
        if separator == '\\' {
            pattern.push('\\');
        }
        pattern.push(separator);
    }
    pattern.push('%');
    pattern
}

fn folder_display_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

struct ItemSnapshot<'a> {
    title: &'a str,
    artist: &'a str,
    album: Option<&'a str>,
    duration_ms: Option<i64>,
    cover_art_id: Option<i64>,
}

struct BindingFacts {
    source_id: i64,
    local: bool,
    enabled: bool,
    available: bool,
    present: bool,
    last_seen_scan: i64,
    file_size: Option<i64>,
    start_offset_ms: i64,
    cue: bool,
}

struct Identity {
    item_id: i64,
    title: String,
    artist: String,
    album: Option<String>,
    duration_ms: Option<i64>,
    bindings: Vec<BindingFacts>,
    cherished: bool,
}

impl Identity {
    fn descriptor(&self, occupies: &dyn Fn(&BindingFacts) -> bool) -> Descriptor {
        let mut sources: Vec<i64> = self
            .bindings
            .iter()
            .filter(|b| occupies(b))
            .map(|b| b.source_id)
            .collect();
        sources.sort_unstable();
        sources.dedup();
        Descriptor {
            item_id: self.item_id,
            title: self.title.clone(),
            artist: self.artist.clone(),
            artist_aliases: Vec::new(),
            album: self.album.clone(),
            duration_ms: self.duration_ms,
            files: self
                .bindings
                .iter()
                .filter_map(|b| {
                    b.file_size
                        .map(|size| (size, file_piece(b.cue, b.start_offset_ms)))
                })
                .collect(),
            live: !sources.is_empty(),
            sources,
        }
    }

    fn playable(&self) -> bool {
        self.bindings
            .iter()
            .any(|b| b.enabled && b.available && b.present)
    }
}

fn file_piece(cue: bool, start_offset_ms: i64) -> i64 {
    if cue { start_offset_ms } else { WHOLE_FILE }
}

fn load_identities(conn: &Connection) -> Result<Vec<Identity>> {
    let mut identities: Vec<Identity> = {
        let mut stmt = conn.prepare(IDENTITY_ITEMS)?;
        stmt.query_map([], |row| {
            Ok(Identity {
                item_id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                album: row.get(3)?,
                duration_ms: row.get(4)?,
                bindings: Vec::new(),
                cherished: false,
            })
        })?
        .collect::<std::result::Result<_, _>>()?
    };
    let by_item: HashMap<i64, usize> = identities
        .iter()
        .enumerate()
        .map(|(ix, identity)| (identity.item_id, ix))
        .collect();
    {
        let mut stmt = conn.prepare(IDENTITY_BINDINGS)?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                BindingFacts {
                    source_id: row.get(1)?,
                    local: row.get(2)?,
                    enabled: row.get(3)?,
                    available: row.get(4)?,
                    present: row.get(5)?,
                    last_seen_scan: row.get(6)?,
                    file_size: row.get(7)?,
                    start_offset_ms: row.get(8)?,
                    cue: row.get(9)?,
                },
            ))
        })?;
        for row in rows {
            let (item_id, facts) = row?;
            if let Some(&ix) = by_item.get(&item_id) {
                identities[ix].bindings.push(facts);
            }
        }
    }
    let mut stmt = conn.prepare(ITEMS_WITH_USER_DATA)?;
    let cherished = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    for item_id in cherished {
        if let Some(&ix) = by_item.get(&item_id?) {
            identities[ix].cherished = true;
        }
    }
    Ok(identities)
}

struct BindingSpec<'a> {
    source_id: i64,
    key: &'a str,
    start_offset_ms: i64,
    file_size: Option<i64>,
    scan_id: i64,
}

fn bind_item(conn: &Connection, item_id: i64, spec: &BindingSpec<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO media_bindings \
         (item_id, source_id, source_key, start_offset_ms, present, last_seen_scan, last_seen_at, \
          file_size) \
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
        rusqlite::params![
            item_id,
            spec.source_id,
            spec.key,
            spec.start_offset_ms,
            spec.scan_id,
            unix_now(),
            spec.file_size
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn create_item(
    conn: &Connection,
    spec: &BindingSpec<'_>,
    snapshot: &ItemSnapshot<'_>,
) -> Result<(i64, i64)> {
    let now = unix_now();
    conn.execute(
        "INSERT INTO media_items \
         (kind, title, artist, album, duration_ms, cover_art_id, created_at, updated_at) \
         VALUES ('track', ?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![
            snapshot.title,
            snapshot.artist,
            snapshot.album,
            snapshot.duration_ms,
            snapshot.cover_art_id,
            now
        ],
    )?;
    let item_id = conn.last_insert_rowid();
    let binding_id = bind_item(conn, item_id, spec)?;
    Ok((binding_id, item_id))
}

fn resolve_local_item(
    conn: &Connection,
    path: &str,
    start_offset_ms: i64,
    snapshot: &ItemSnapshot<'_>,
) -> Result<i64> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT b.id, b.item_id FROM media_bindings b \
             JOIN sources s ON s.id = b.source_id \
             WHERE s.kind = 'local' AND b.source_key = ?1 AND b.start_offset_ms = ?2 \
             LIMIT 1",
            rusqlite::params![path, start_offset_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((binding_id, item_id)) = existing {
        conn.execute(
            "UPDATE media_bindings SET present = 1, last_seen_at = ?1 WHERE id = ?2",
            rusqlite::params![unix_now(), binding_id],
        )?;
        return Ok(item_id);
    }
    let roots = load_local_roots(conn)?;
    let source_id = source_for_path(&roots, path);
    let (_, item_id) = create_item(
        conn,
        &BindingSpec {
            source_id,
            key: path,
            start_offset_ms,
            file_size: None,
            scan_id: 0,
        },
        snapshot,
    )?;
    Ok(item_id)
}

impl SqliteLibrary {
    pub fn open() -> Result<Self> {
        // `PULSE_HOME` is what makes an exported copy independent; unset, this
        // is the platform data dir exactly as before.
        let db_dir = match std::env::var_os("PULSE_HOME") {
            Some(home) if !home.is_empty() => PathBuf::from(home),
            _ => dirs::data_dir()
                .ok_or_else(|| LibraryError::InvalidData("no data dir".into()))?
                .join("pulse-music"),
        };
        std::fs::create_dir_all(&db_dir)?;

        let db_path = db_dir.join("library.db");
        // Before anything opens a connection: a restore requested from the UI is
        // applied here, because this is the only safe moment to swap the file.
        if let Err(err) = apply_pending_restore(&db_path) {
            log::error!("library: could not apply the pending restore: {err}");
        }
        if db_path.exists() && premigration_schema(&db_path) {
            let check = Connection::open(&db_path);
            if let Ok(check_conn) = check {
                let has_cover_art: bool = check_conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='cover_art')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);
                let has_liked: bool = check_conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tracks') WHERE name='liked')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);
                let has_playlists: bool = check_conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='playlists')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);
                let has_playlist_unique: bool = check_conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_playlist_tracks_pair')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);
                let has_scan_meta: bool = check_conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='scan_meta')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);
                let has_bitrate: bool = check_conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tracks') WHERE name='bitrate')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);
                drop(check_conn);
                if !has_cover_art
                    || !has_liked
                    || !has_playlists
                    || !has_playlist_unique
                    || !has_scan_meta
                    || !has_bitrate
                {
                    remove_db_files(&db_path);
                }
            }
        }

        let conn = Connection::open(&db_path)?;
        apply_pragmas(&conn)?;
        let lib = Self {
            conn: Mutex::new(conn),
            db_path,
        };
        lib.run_migrations()?;
        Ok(lib)
    }

    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let db_dir = path.parent().unwrap_or(path);
        std::fs::create_dir_all(db_dir)?;
        if let Err(err) = apply_pending_restore(path) {
            log::error!("library: could not apply the pending restore: {err}");
        }
        let conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        let lib = Self {
            conn: Mutex::new(conn),
            db_path: path.to_path_buf(),
        };
        lib.run_migrations()?;
        Ok(lib)
    }

    fn run_migrations(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let user_version: i32 =
            conn.query_row("SELECT user_version FROM pragma_user_version", [], |row| {
                row.get(0)
            })?;
        let latest = MIGRATIONS.last().map_or(0, |(version, _)| *version);
        if user_version >= latest {
            return Ok(());
        }
        if user_version > 0 && user_version < latest {
            // Any upgrade of a database that already holds something gets a
            // restorable copy first. This deliberately does not special-case the
            // identity migration any more: a later migration that drops tables
            // (the Liked removal) needs the same protection.
            backup_before_migration(&conn, &self.db_path, user_version)?;
        }
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        let applied = apply_migrations(&mut conn, user_version);
        conn.pragma_update(None, "foreign_keys", "ON")?;
        applied
    }

    /// Take today's rotated backup, if one has not been taken yet, then prune
    /// the older ones.
    ///
    /// Called once per launch by the app, before anything else in the session
    /// can write to the database. Synchronous on purpose: the first launch of
    /// the day pays a few hundred milliseconds, and in exchange the copy is
    /// taken while nothing else is touching the file. Deliberately *not* called
    /// from [`Self::open`] / [`Self::open_at`], so tests never litter a backup
    /// directory.
    ///
    /// Returns the path written, or `None` when today's backup already exists.
    pub fn daily_backup(&self, retain: usize) -> Result<Option<PathBuf>> {
        let Some((dir, stem)) = backup::backup_location(&self.db_path) else {
            return Ok(None);
        };
        std::fs::create_dir_all(&dir)?;
        let name = backup::backup_file_name(&stem, unix_now());
        let target = dir.join(&name);

        let written = if target.exists() {
            None
        } else {
            // `VACUUM INTO`, never a file copy: the database is in WAL mode, so
            // copying it while a `-wal` is populated can produce a file that is
            // missing the newest transactions or does not open at all.
            //
            // Into `<name>.partial` and then renamed, so a crash or a full disk
            // cannot leave a truncated file wearing a name that looks like a
            // good backup.
            let partial_name = format!("{name}.partial");
            remove_partial_copy(&dir, &partial_name);
            let partial = dir.join(&partial_name);
            {
                let conn = self.conn.lock().unwrap();
                conn.execute("VACUUM INTO ?1", [partial.to_string_lossy().into_owned()])?;
            }
            std::fs::rename(&partial, &target)?;
            Some(target.clone())
        };

        prune_backups(&dir, &stem, retain);
        Ok(written)
    }

    fn get_or_insert_artist(&self, tx: &rusqlite::Transaction, name: &str) -> Result<i64> {
        let sort_name = compute_sort_name(name);
        let key = artist_key(name);
        let existing = {
            let mut stmt = tx.prepare("SELECT id, name FROM artists ORDER BY id")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut found = None;
            for row in rows {
                let (id, existing) = row?;
                if artist_key(&existing) == key {
                    found = Some(id);
                    break;
                }
            }
            found
        };
        if let Some(id) = existing {
            return Ok(id);
        }
        let id = allocate_catalog_id(tx, "artists", ARTIST_ID_HIGH)?;
        tx.execute(
            "INSERT INTO artists (id, name, sort_name) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, sort_name],
        )?;
        Ok(id)
    }
}

impl LibraryRepository for SqliteLibrary {
    fn upsert_artist(&self, name: &str) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let id = self.get_or_insert_artist(&tx, name)?;
        tx.commit()?;
        Ok(id)
    }

    fn upsert_album(
        &self,
        title: &str,
        year: Option<i32>,
        cover_art_id: Option<i64>,
    ) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if let Some(id) = tx
            .query_row(
                "SELECT id FROM albums WHERE title = ?1 AND (year IS ?2)",
                rusqlite::params![title, year],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            if cover_art_id.is_some() {
                tx.execute(
                    "UPDATE albums SET cover_art_id = ?1 WHERE id = ?2",
                    rusqlite::params![cover_art_id, id],
                )?;
            }
            tx.commit()?;
            return Ok(id);
        }
        let id = allocate_catalog_id(&tx, "albums", ALBUM_ID_HIGH)?;
        tx.execute(
            "INSERT INTO albums (id, title, year, cover_art_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, title, year, cover_art_id],
        )?;
        tx.commit()?;
        Ok(id)
    }

    fn set_album_artists(&self, album_id: i64, artist_ids: &[(i64, i32)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM album_artists WHERE album_id = ?1", [album_id])?;
        for (artist_id, position) in artist_ids {
            tx.execute(
                "INSERT INTO album_artists (album_id, artist_id, position) VALUES (?1, ?2, ?3)",
                [album_id, *artist_id, *position as i64],
            )?;
        }
        tx.execute(
            "UPDATE albums SET artist_known = ?2 WHERE id = ?1",
            [album_id, !artist_ids.is_empty() as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn set_track_album_artists(&self, track_id: i64, artist_ids: &[(i64, i32)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM track_album_artists WHERE track_id = ?1",
            [track_id],
        )?;
        for (artist_id, position) in artist_ids {
            tx.execute(
                "INSERT OR IGNORE INTO track_album_artists (track_id, artist_id, position) VALUES (?1, ?2, ?3)",
                [track_id, *artist_id, *position as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn track_album_artists(&self, track_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT a.name
            FROM artists a
            JOIN track_album_artists x ON x.artist_id = a.id
            WHERE x.track_id = ?1
            ORDER BY x.position
            "#,
        )?;
        let rows = stmt.query_map([track_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn upsert_track(
        &self,
        track: &NewTrack,
        album_id: Option<i64>,
        artist_ids: &[(i64, i32)],
    ) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let title = track
            .title
            .clone()
            .unwrap_or_else(|| fallback_title_from_path(&track.path));
        let track_number = track.track_number.map(|n| n as i32);
        let disc_number = track.disc_number.unwrap_or(1) as i32;
        let duration_ms = track.duration_ms.map(|n| n as i64);
        let start_offset_ms = track.start_offset_ms.unwrap_or(0) as i32;

        let existing_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1 AND start_offset_ms = ?2",
                rusqlite::params![track.path, start_offset_ms],
                |row| row.get(0),
            )
            .optional()?;

        let track_id = if let Some(id) = existing_id {
            tx.execute(
                r#"UPDATE tracks SET
                    title = ?1,
                    album_id = ?2,
                    track_number = ?3,
                    disc_number = ?4,
                    duration_ms = ?5,
                    year = ?6,
                    cover_art_id = COALESCE(?7, cover_art_id),
                    start_offset_ms = ?8,
                    bitrate = ?10
                WHERE id = ?9"#,
                rusqlite::params![
                    title,
                    album_id,
                    track_number,
                    disc_number,
                    duration_ms,
                    track.year,
                    track.cover_art_id,
                    start_offset_ms,
                    id,
                    track.bitrate,
                ],
            )?;
            id
        } else {
            let album_title = match album_id {
                Some(id) => tx
                    .query_row("SELECT title FROM albums WHERE id = ?1", [id], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?,
                None => None,
            };
            let artist = match artist_ids.first() {
                Some((id, _)) => tx
                    .query_row("SELECT name FROM artists WHERE id = ?1", [id], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                    .unwrap_or_default(),
                None => String::new(),
            };
            let item_id = resolve_local_item(
                &tx,
                &track.path,
                start_offset_ms as i64,
                &ItemSnapshot {
                    title: &title,
                    artist: &artist,
                    album: album_title.as_deref(),
                    duration_ms,
                    cover_art_id: track.cover_art_id,
                },
            )?;
            tx.execute(
                r#"INSERT INTO tracks
                    (id, path, title, album_id, track_number, disc_number, duration_ms, year, cover_art_id, start_offset_ms, bitrate)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
                rusqlite::params![
                    item_id,
                    track.path,
                    title,
                    album_id,
                    track_number,
                    disc_number,
                    duration_ms,
                    track.year,
                    track.cover_art_id,
                    start_offset_ms,
                    track.bitrate,
                ],
            )?;
            item_id
        };

        tx.execute("DELETE FROM track_artists WHERE track_id = ?1", [track_id])?;
        for (artist_id, position) in artist_ids {
            tx.execute(
                "INSERT INTO track_artists (track_id, artist_id, role, position) VALUES (?1, ?2, 'main', ?3)",
                [track_id, *artist_id, *position as i64],
            )?;
        }

        tx.commit()?;
        Ok(track_id)
    }

    fn albums(&self) -> Result<Vec<AlbumSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT
                a.id,
                a.title,
                a.year,
                a.cover_art_id,
                art.name,
                art.id
            FROM albums a
            LEFT JOIN album_artists aa ON aa.album_id = a.id AND aa.position = 0
            LEFT JOIN artists art ON art.id = aa.artist_id
            ORDER BY COALESCE(NULLIF(art.sort_name, ''), art.name), a.year, a.title
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AlbumSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                year: row.get(2)?,
                cover_art_id: row.get(3)?,
                artist_name: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                artist_id: row.get::<_, Option<i64>>(5)?,
            })
        })?;
        let mut albums = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)?;
        let has_orphans: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tracks WHERE album_id IS NULL)",
            [],
            |row| row.get(0),
        )?;
        if has_orphans {
            albums.push(AlbumSummary {
                id: crate::NO_METADATA_ALBUM_ID,
                title: String::new(),
                year: None,
                cover_art_id: None,
                artist_name: String::new(),
                artist_id: None,
            });
        }
        Ok(albums)
    }

    fn album_search_entries(&self) -> Result<Vec<AlbumSearchEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                a.id,
                a.title || ' ' ||
                COALESCE(artists_concat.names, '') || ' ' ||
                COALESCE(track_artists_concat.names, '') || ' ' ||
                COALESCE(tracks_concat.titles, '')
            FROM albums a
            LEFT JOIN (
                SELECT aa.album_id AS album_id, GROUP_CONCAT(art.name, ' ') AS names
                FROM album_artists aa
                JOIN artists art ON art.id = aa.artist_id
                GROUP BY aa.album_id
            ) artists_concat ON artists_concat.album_id = a.id
            LEFT JOIN (
                SELECT t.album_id AS album_id, GROUP_CONCAT(DISTINCT art.name) AS names
                FROM tracks t
                JOIN track_artists ta ON ta.track_id = t.id
                JOIN artists art ON art.id = ta.artist_id
                WHERE t.album_id IS NOT NULL
                GROUP BY t.album_id
            ) track_artists_concat ON track_artists_concat.album_id = a.id
            LEFT JOIN (
                SELECT album_id, GROUP_CONCAT(title, ' ') AS titles
                FROM tracks
                WHERE album_id IS NOT NULL AND title IS NOT NULL
                GROUP BY album_id
            ) tracks_concat ON tracks_concat.album_id = a.id
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AlbumSearchEntry {
                album_id: row.get(0)?,
                haystack: row.get(1)?,
            })
        })?;
        let mut entries = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)?;
        let orphan_titles: Option<String> = conn.query_row(
            "SELECT GROUP_CONCAT(title, ' ') FROM tracks WHERE album_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        if let Some(haystack) = orphan_titles {
            entries.push(AlbumSearchEntry {
                album_id: crate::NO_METADATA_ALBUM_ID,
                haystack,
            });
        }
        Ok(entries)
    }

    fn tracks_for_album(&self, album_id: i64) -> Result<Vec<Track>> {
        let conn = self.conn.lock().unwrap();
        if album_id == crate::NO_METADATA_ALBUM_ID {
            let sql = format!(
                "SELECT {TRACK_COLUMNS} FROM tracks WHERE album_id IS NULL \
                 ORDER BY disc_number, track_number, title, id",
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let rows = stmt.query_map([], map_track_row)?;
            return rows
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(LibraryError::Database);
        }
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM tracks WHERE album_id = ?1 \
             ORDER BY disc_number, track_number, title, id",
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([album_id], map_track_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn album_track_counts(&self) -> Result<HashMap<i64, i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT album_id, COUNT(*) FROM tracks WHERE album_id IS NOT NULL GROUP BY album_id",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(LibraryError::Database)
    }

    fn track_artists(&self, track_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT a.name
            FROM artists a
            JOIN track_artists ta ON ta.artist_id = a.id
            WHERE ta.track_id = ?1
            ORDER BY ta.position
            "#,
        )?;
        let rows = stmt.query_map([track_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn track_artists_with_ids(&self, track_id: i64) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT a.id, a.name
            FROM artists a
            JOIN track_artists ta ON ta.artist_id = a.id
            WHERE ta.track_id = ?1
            ORDER BY ta.position
            "#,
        )?;
        let rows = stmt.query_map([track_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn album_title(&self, album_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT title FROM albums WHERE id = ?1")?;
        let title = stmt
            .query_row([album_id], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten();
        Ok(title)
    }

    fn album_genres(&self, album_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT g.name
            FROM genres g
            JOIN track_genres tg ON tg.genre_id = g.id
            JOIN tracks t ON t.id = tg.track_id
            WHERE t.album_id = ?1
            GROUP BY g.id
            ORDER BY COUNT(*) DESC, g.name
            "#,
        )?;
        let rows = stmt.query_map([album_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn track_genres(&self, track_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT g.name
            FROM genres g
            JOIN track_genres tg ON tg.genre_id = g.id
            WHERE tg.track_id = ?1
            ORDER BY tg.position
            "#,
        )?;
        let rows = stmt.query_map([track_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn album_artists(&self, album_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT ar.name
            FROM album_artists aa
            JOIN artists ar ON ar.id = aa.artist_id
            WHERE aa.album_id = ?1
            ORDER BY aa.position
            "#,
        )?;
        let rows = stmt.query_map([album_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn album_genres_map(&self) -> Result<HashMap<i64, Vec<String>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT t.album_id, g.name
            FROM genres g
            JOIN track_genres tg ON tg.genre_id = g.id
            JOIN tracks t ON t.id = tg.track_id
            WHERE t.album_id IS NOT NULL
            GROUP BY t.album_id, g.id
            ORDER BY t.album_id, COUNT(*) DESC, g.name
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map: HashMap<i64, Vec<String>> = HashMap::new();
        for row in rows {
            let (album_id, name) = row.map_err(LibraryError::Database)?;
            map.entry(album_id).or_default().push(name);
        }
        Ok(map)
    }

    fn clear(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute_batch(CLEAR_CATALOG)?;
        tx.commit()?;
        Ok(())
    }

    fn has_tracks(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM tracks)", [], |row| row.get(0))?;
        Ok(exists)
    }

    fn delete_orphaned_albums_and_artists(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM albums WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
            [],
        )?;
        tx.execute(
            "DELETE FROM artists WHERE NOT EXISTS (SELECT 1 FROM album_artists WHERE album_artists.artist_id = artists.id)
             AND NOT EXISTS (SELECT 1 FROM track_artists WHERE track_artists.artist_id = artists.id)
             AND NOT EXISTS (SELECT 1 FROM track_album_artists WHERE track_album_artists.artist_id = artists.id)",
            [],
        )?;
        tx.execute(
            "DELETE FROM cover_art WHERE NOT EXISTS (SELECT 1 FROM albums WHERE albums.cover_art_id = cover_art.id)
             AND NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.cover_art_id = cover_art.id)
             AND NOT EXISTS (SELECT 1 FROM media_items WHERE media_items.cover_art_id = cover_art.id)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn save_cover_art(&self, data: &[u8], source_path: &str, embedded: bool) -> Result<i64> {
        let hash = compute_sha256(data);

        {
            let conn = self.conn.lock().unwrap();
            if let Some(id) = conn
                .query_row("SELECT id FROM cover_art WHERE hash = ?1", [&hash], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()?
            {
                return Ok(id);
            }
        }

        let thumbnails = crate::thumbnail::generate_thumbnails(data)?;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO cover_art (hash, small, large, source_path, embedded) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                hash,
                thumbnails.small,
                thumbnails.large,
                source_path,
                embedded
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    fn set_track_cover(&self, track_id: i64, cover_art_id: Option<i64>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tracks SET cover_art_id = ?1 WHERE id = ?2",
            rusqlite::params![cover_art_id, track_id],
        )?;
        Ok(())
    }

    fn get_cover_art(&self, id: i64) -> Result<Option<CoverArt>> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row(
                "SELECT id, small, large FROM cover_art WHERE id = ?1",
                [id],
                |row| {
                    Ok(CoverArt {
                        id: row.get(0)?,
                        small: row.get(1)?,
                        large: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    fn get_cover_art_small(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row("SELECT small FROM cover_art WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(result)
    }

    fn get_cover_art_large(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row("SELECT large FROM cover_art WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(result)
    }

    fn get_cover_art_source(&self, id: i64) -> Result<Option<(String, bool)>> {
        let conn = self.conn.lock().unwrap();
        let result: Option<(Option<String>, bool)> = conn
            .query_row(
                "SELECT source_path, embedded FROM cover_art WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(result.and_then(|(path, embedded)| path.map(|p| (p, embedded))))
    }

    fn get_track_path_for_cover(&self, id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row(
                "SELECT path FROM tracks WHERE cover_art_id = ?1 LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    fn resolve_album_covers(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE albums SET cover_art_id = (
                SELECT t.cover_art_id FROM tracks t
                WHERE t.album_id = albums.id AND t.cover_art_id IS NOT NULL
                ORDER BY t.disc_number, COALESCE(t.track_number, 999999), t.path
                LIMIT 1
            )
            "#,
            [],
        )?;
        Ok(())
    }

    fn resolve_album_artists(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut explicit: HashMap<i64, Vec<i64>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT track_id, artist_id FROM track_album_artists ORDER BY track_id, position",
            )?;
            let rows =
                stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            for row in rows {
                let (track_id, artist_id) = row?;
                explicit.entry(track_id).or_default().push(artist_id);
            }
        }
        let mut credited: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT ta.track_id, a.id, a.name FROM track_artists ta \
                 JOIN artists a ON a.id = ta.artist_id ORDER BY ta.track_id, ta.position",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (track_id, artist_id, name) = row?;
                credited
                    .entry(track_id)
                    .or_default()
                    .push((artist_id, name));
            }
        }
        let mut albums: Vec<(i64, Vec<AlbumTrackArtists>)> = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT t.id, t.album_id FROM tracks t WHERE t.album_id IS NOT NULL \
                 ORDER BY t.album_id, t.disc_number, COALESCE(t.track_number, 999999), t.path",
            )?;
            let rows =
                stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            for row in rows {
                let (track_id, album_id) = row?;
                let entry = AlbumTrackArtists {
                    explicit: explicit.remove(&track_id).unwrap_or_default(),
                    artists: credited.remove(&track_id).unwrap_or_default(),
                };
                match albums.last_mut() {
                    Some((id, tracks)) if *id == album_id => tracks.push(entry),
                    _ => albums.push((album_id, vec![entry])),
                }
            }
        }

        tx.execute("DELETE FROM album_artists", [])?;
        tx.execute("UPDATE albums SET artist_known = 0", [])?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO album_artists (album_id, artist_id, position) VALUES (?1, ?2, ?3)",
            )?;
            let mut mark_known = tx.prepare("UPDATE albums SET artist_known = 1 WHERE id = ?1")?;
            for (album_id, tracks) in &albums {
                let derived = derive_album_artists(tracks);
                for (position, artist_id) in derived.artist_ids.iter().enumerate() {
                    insert.execute([*album_id, *artist_id, position as i64])?;
                }
                if derived.known {
                    mark_known.execute([*album_id])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn album_artist_known(&self, album_id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT artist_known FROM albums WHERE id = ?1",
            [album_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|known| known.unwrap_or(0) != 0)
        .map_err(LibraryError::Database)
    }

    fn artists(&self, grouping: ArtistGrouping) -> Result<Vec<ArtistSummary>> {
        let conn = self.conn.lock().unwrap();
        let ctes = membership_ctes(grouping);
        let sql = format!(
            "WITH {ctes} \
             SELECT a.id, a.name, a.sort_name, COUNT(DISTINCT u.track_id) AS track_count \
             FROM artists a \
             JOIN u ON u.artist_id = a.id \
             WHERE EXISTS (SELECT 1 FROM m WHERE m.artist_id = a.id) \
             GROUP BY a.id \
             HAVING track_count > 0 \
             ORDER BY a.sort_name COLLATE NOCASE, a.name COLLATE NOCASE"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], map_artist_summary)?;
        let mut artists = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)?;
        let orphan_count = orphan_track_count(&conn, grouping)?;
        if orphan_count > 0 {
            artists.push(no_metadata_artist(orphan_count));
        }
        Ok(artists)
    }

    fn artist_summary(&self, id: i64, grouping: ArtistGrouping) -> Result<Option<ArtistSummary>> {
        let conn = self.conn.lock().unwrap();
        if id == crate::NO_METADATA_ARTIST_ID {
            let orphan_count = orphan_track_count(&conn, grouping)?;
            return Ok((orphan_count > 0).then(|| no_metadata_artist(orphan_count)));
        }
        let ctes = membership_ctes(grouping);
        let sql = format!(
            "WITH {ctes} \
             SELECT a.id, a.name, a.sort_name, COUNT(DISTINCT u.track_id) AS track_count \
             FROM artists a \
             JOIN u ON u.artist_id = a.id \
             WHERE a.id = ?1 AND EXISTS (SELECT 1 FROM m WHERE m.artist_id = a.id) \
             GROUP BY a.id \
             HAVING track_count > 0"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        stmt.query_row([id], map_artist_summary)
            .optional()
            .map_err(LibraryError::Database)
    }

    fn listed_artist_for_credit(
        &self,
        track_id: i64,
        credited_artist_id: i64,
        grouping: ArtistGrouping,
    ) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let credited: Option<String> = conn
            .query_row(
                "SELECT name FROM artists WHERE id = ?1",
                [credited_artist_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(credited) = credited else {
            return Ok(None);
        };
        let listed = artist_membership_sql(grouping);
        let sql = format!(
            "WITH m AS ({listed}) \
             SELECT a.id, a.name FROM m JOIN artists a ON a.id = m.artist_id \
             WHERE m.track_id = ?1 ORDER BY m.position, a.id"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([track_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row.map_err(LibraryError::Database)?;
            if crate::album_artists::is_credit_of(&name, &credited) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    fn artist_search_haystacks(&self, grouping: ArtistGrouping) -> Result<HashMap<i64, String>> {
        let conn = self.conn.lock().unwrap();
        let ctes = membership_ctes(grouping);
        let sql = match grouping {
            ArtistGrouping::TrackArtist => format!(
                "WITH {ctes} \
                 SELECT a.id, a.name FROM artists a \
                 WHERE EXISTS (SELECT 1 FROM m WHERE m.artist_id = a.id)"
            ),
            ArtistGrouping::AlbumArtist => format!(
                "WITH {ctes} \
                 SELECT a.id, a.name || ' ' || COALESCE(names.list, '') \
                 FROM artists a \
                 LEFT JOIN ( \
                     SELECT u.artist_id, GROUP_CONCAT(DISTINCT art.name) AS list \
                     FROM u \
                     JOIN track_artists ta ON ta.track_id = u.track_id \
                     JOIN artists art ON art.id = ta.artist_id \
                     WHERE art.id != u.artist_id \
                     GROUP BY u.artist_id \
                 ) names ON names.artist_id = a.id \
                 WHERE EXISTS (SELECT 1 FROM m WHERE m.artist_id = a.id)"
            ),
        };
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(LibraryError::Database)
    }

    fn artist_name(&self, id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT name FROM artists WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(LibraryError::Database)
    }

    fn tracks_by_artist(&self, artist_id: i64, grouping: ArtistGrouping) -> Result<Vec<Track>> {
        let conn = self.conn.lock().unwrap();
        let listed = artist_membership_sql(grouping);
        if artist_id == crate::NO_METADATA_ARTIST_ID {
            let sql = format!(
                "WITH m AS ({listed}) \
                 SELECT {TRACK_COLUMNS_T} FROM tracks t \
                 LEFT JOIN albums al ON al.id = t.album_id \
                 WHERE NOT EXISTS (SELECT 1 FROM m WHERE m.track_id = t.id) \
                 ORDER BY COALESCE(al.year, 0), al.title COLLATE NOCASE, {ALBUM_RUN_TIEBREAK}",
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let rows = stmt.query_map([], map_track_row)?;
            return rows
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(LibraryError::Database);
        }
        let ctes = membership_ctes(grouping);
        let sql = format!(
            "WITH {ctes} \
             SELECT DISTINCT {TRACK_COLUMNS_T} FROM tracks t \
             JOIN u ON u.track_id = t.id \
             LEFT JOIN albums al ON al.id = t.album_id \
             WHERE u.artist_id = ?1 \
             ORDER BY COALESCE(al.year, 0), al.title COLLATE NOCASE, {ALBUM_RUN_TIEBREAK}",
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([artist_id], map_track_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn folder_roots(&self) -> Result<Vec<FolderSummary>> {
        let conn = self.conn.lock().unwrap();
        let roots = load_local_roots(&conn)?;
        let mut summaries = Vec::new();
        for root in roots {
            let path = clean_dir(&root.path.to_string_lossy());
            if path.is_empty() {
                continue;
            }
            let pattern = folder_like_pattern(&path);
            let mut stmt = conn.prepare_cached(
                "SELECT COUNT(*), COUNT(DISTINCT album_id) FROM tracks \
                 WHERE path LIKE ?1 ESCAPE '\\'",
            )?;
            let (track_count, album_count): (i64, i64) = stmt
                .query_row([&pattern], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(LibraryError::Database)?;
            // A root with nothing playable under it is not worth showing.
            if track_count == 0 {
                continue;
            }
            summaries.push(FolderSummary {
                name: folder_display_name(&path),
                path,
                track_count,
                album_count,
            });
        }
        summaries.sort_by_key(|s| s.name.to_lowercase());
        Ok(summaries)
    }

    fn child_folders(&self, path: &str) -> Result<Vec<FolderSummary>> {
        let conn = self.conn.lock().unwrap();
        let base = clean_dir(path);
        let base_path = PathBuf::from(&base);
        let mut stmt = conn
            .prepare_cached("SELECT path, album_id FROM tracks WHERE path LIKE ?1 ESCAPE '\\'")?;
        let rows = stmt
            .query_map([folder_like_pattern(&base)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .map_err(LibraryError::Database)?;

        let mut folders: BTreeMap<String, (i64, HashSet<Option<i64>>)> = BTreeMap::new();
        for row in rows {
            let (track_path, album_id) = row.map_err(LibraryError::Database)?;
            let Ok(rest) = Path::new(&track_path).strip_prefix(&base_path) else {
                continue;
            };
            let mut components = rest.components();
            let Some(first) = components.next() else {
                continue;
            };
            // A single remaining component means the file sits directly in
            // `base` rather than in a subdirectory of it.
            if components.next().is_none() {
                continue;
            }
            let name = first.as_os_str().to_string_lossy().into_owned();
            if name.is_empty() {
                continue;
            }
            let entry = folders.entry(name).or_insert_with(|| (0, HashSet::new()));
            entry.0 += 1;
            entry.1.insert(album_id);
        }

        Ok({
            let mut summaries: Vec<FolderSummary> = folders
                .into_iter()
                .map(|(name, (track_count, albums))| FolderSummary {
                    // The host separator, because the track paths these come from
                    // are host-native; joining with a literal `/` built folder
                    // paths that no stored path could ever match on Windows.
                    path: format!("{base}{}{name}", std::path::MAIN_SEPARATOR),
                    name,
                    track_count,
                    // `None` is "tracks with no album", not an album. Counting it
                    // claimed one album for a folder of loose files.
                    album_count: albums.iter().filter(|id| id.is_some()).count() as i64,
                })
                .collect();
            // `folder_roots` orders case-insensitively, and a level that ordered
            // differently put "apple" after "Zebra" on every level below the top.
            summaries.sort_by_key(|s| (s.name.to_lowercase(), s.name.clone()));
            summaries
        })
    }

    fn tracks_in_folder(&self, path: &str) -> Result<Vec<Track>> {
        let conn = self.conn.lock().unwrap();
        let pattern = folder_like_pattern(&clean_dir(path));
        let sql = format!(
            "SELECT {TRACK_COLUMNS_T} FROM tracks t \
             LEFT JOIN albums al ON al.id = t.album_id \
             LEFT JOIN album_artists aa ON aa.album_id = al.id AND aa.position = 0 \
             LEFT JOIN artists art ON art.id = aa.artist_id \
             WHERE t.path LIKE ?1 ESCAPE '\\' \
             ORDER BY art.sort_name COLLATE NOCASE, COALESCE(al.year, 0), \
             al.title COLLATE NOCASE, {ALBUM_RUN_TIEBREAK}",
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([pattern], map_track_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn tracks_directly_in_folder(&self, path: &str) -> Result<Vec<Track>> {
        let base = clean_dir(path);
        Ok(self
            .tracks_in_folder(&base)?
            .into_iter()
            // `tracks_in_folder` already returns the display order, so this only
            // drops the rows that belong to a subdirectory.
            .filter(|track| {
                std::path::Path::new(&track.path).parent() == Some(std::path::Path::new(&base))
            })
            .collect())
    }

    fn direct_track_count(&self, path: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let base = clean_dir(path);
        let mut stmt = conn
            .prepare_cached("SELECT path FROM tracks WHERE path LIKE ?1 ESCAPE '\\'")?;
        let rows = stmt.query_map([folder_like_pattern(&base)], |row| row.get::<_, String>(0))?;
        let mut count = 0i64;
        for row in rows {
            let track_path = row.map_err(LibraryError::Database)?;
            // Directly in the folder, not one of its subdirectories: the LIKE
            // already restricts us to the subtree, so this is the whole test.
            if std::path::Path::new(&track_path).parent() == Some(std::path::Path::new(&base)) {
                count += 1;
            }
        }
        Ok(count)
    }

    fn replaygain(&self, track_id: i64) -> Result<Option<crate::ReplayGain>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT track_gain, track_peak, album_gain, album_peak \
             FROM track_gain WHERE track_id = ?1",
        )?;
        let mut rows = stmt.query_map([track_id], |row| {
            Ok(crate::ReplayGain {
                track_gain: row.get::<_, Option<f64>>(0)?.map(|v| v as f32),
                track_peak: row.get::<_, Option<f64>>(1)?.map(|v| v as f32),
                album_gain: row.get::<_, Option<f64>>(2)?.map(|v| v as f32),
                album_peak: row.get::<_, Option<f64>>(3)?.map(|v| v as f32),
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(LibraryError::Database)?)),
            None => Ok(None),
        }
    }

    fn track(&self, id: i64) -> Result<Option<Track>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1");
        let mut stmt = conn.prepare_cached(&sql)?;
        let mut rows = stmt.query_map([id], map_track_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(LibraryError::Database)?)),
            None => Ok(None),
        }
    }

    fn tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Dedupe and chunk: SQLite refuses a statement with more bound
        // parameters than SQLITE_MAX_VARIABLE_NUMBER, and a restored queue of a
        // large library is far past that.
        let mut ids: Vec<i64> = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        for chunk in ids.chunks(512) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id IN ({placeholders})");
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(params.as_slice(), map_track_row)?;
            for row in rows {
                out.push(row.map_err(LibraryError::Database)?);
            }
        }
        Ok(out)
    }

    fn all_tracks(&self) -> Result<Vec<Track>> {
        let conn = self.conn.lock().unwrap();
        display_ordered_tracks(&conn, "")
    }

    fn track_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT COUNT(*) FROM tracks")?;
        let count = stmt.query_row([], |row| row.get(0))?;
        Ok(count)
    }

    fn set_track_genres(&self, track_id: i64, genres: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM track_genres WHERE track_id = ?1", [track_id])?;

        for (position, name) in genres.iter().enumerate() {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let key = name.to_lowercase();
            tx.execute(
                "INSERT OR IGNORE INTO genres (name, key) VALUES (?1, ?2)",
                [name, key.as_str()],
            )?;
            let genre_id: i64 =
                tx.query_row("SELECT id FROM genres WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })?;
            tx.execute(
                "INSERT OR IGNORE INTO track_genres (track_id, genre_id, position) VALUES (?1, ?2, ?3)",
                [track_id, genre_id, position as i64],
            )?;
        }

        tx.execute(
            "DELETE FROM genres WHERE NOT EXISTS \
             (SELECT 1 FROM track_genres WHERE track_genres.genre_id = genres.id)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn create_playlist(&self, name: &str) -> Result<i64> {
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO playlists (name, created_at) VALUES (?1, ?2)",
            rusqlite::params![name, created_at],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn delete_playlist(&self, playlist_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM playlists WHERE id = ?1", [playlist_id])?;
        Ok(())
    }

    fn rename_playlist(&self, playlist_id: i64, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE playlists SET name = ?2 WHERE id = ?1",
            rusqlite::params![playlist_id, name],
        )?;
        Ok(())
    }

    fn playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT
                p.id,
                p.name,
                p.created_at,
                COUNT(pt.track_id) AS track_count
            FROM playlists p
            LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
            GROUP BY p.id
            ORDER BY p.created_at ASC, p.id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PlaylistSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                track_count: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn add_track_to_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        self.add_tracks_to_playlist(playlist_id, &[track_id])
    }

    fn add_tracks_to_playlist(&self, playlist_id: i64, track_ids: &[i64]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut next_position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1",
                [playlist_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        // INSERT OR IGNORE: the (playlist_id, track_id) UNIQUE index silently
        // dedupes — double-clicks and stale "containing" UI checks become
        // harmless instead of erroring or producing duplicates.
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO playlist_tracks (playlist_id, position, track_id)
                 SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM media_items WHERE id = ?3)",
            )?;
            for &track_id in track_ids {
                if insert.execute(rusqlite::params![playlist_id, next_position, track_id])? > 0 {
                    next_position += 1;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn remove_track_from_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Remove the lowest-position occurrence of the track (Spotify-ish: if
        // the same track is in the playlist multiple times, removes one copy).
        let position: Option<i64> = tx
            .query_row(
                "SELECT MIN(position) FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
                rusqlite::params![playlist_id, track_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let Some(position) = position else {
            tx.commit()?;
            return Ok(());
        };
        tx.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
            rusqlite::params![playlist_id, position],
        )?;
        // Compact positions so they stay dense.
        tx.execute(
            "UPDATE playlist_tracks SET position = position - 1 \
             WHERE playlist_id = ?1 AND position > ?2",
            rusqlite::params![playlist_id, position],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn move_track_in_playlist(&self, playlist_id: i64, from: usize, to: usize) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
            )?;
            let rows = stmt.query_map([playlist_id], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        if from >= ids.len() || to >= ids.len() {
            tx.commit()?;
            return Ok(());
        }
        let moved = ids.remove(from);
        ids.insert(to, moved);
        tx.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            [playlist_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            )?;
            for (position, track_id) in ids.iter().enumerate() {
                stmt.execute(rusqlite::params![playlist_id, position as i64, track_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn tracks_for_playlist(&self, playlist_id: i64) -> Result<Vec<Track>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {PLAYLIST_ENTRY_COLUMNS} FROM playlist_tracks pt \
             JOIN media_items m ON m.id = pt.track_id \
             LEFT JOIN tracks t ON t.id = m.id \
             LEFT JOIN media_bindings b ON b.id = ( \
                 SELECT id FROM media_bindings WHERE item_id = m.id \
                 ORDER BY present DESC, id LIMIT 1) \
             WHERE pt.playlist_id = ?1 \
             ORDER BY pt.position",
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([playlist_id], map_track_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn tracks_by_keys(&self, keys: &[(String, i32)]) -> Result<Vec<Track>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let mut paths: Vec<&str> = keys.iter().map(|(p, _)| p.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        let mut out = Vec::new();
        for chunk in paths.chunks(512) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE path IN ({placeholders})");
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(params.as_slice(), map_track_row)?;
            for row in rows {
                out.push(row.map_err(LibraryError::Database)?);
            }
        }
        Ok(out)
    }

    fn playlists_containing_track(&self, track_id: i64) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT playlist_id FROM playlist_tracks WHERE track_id = ?1",
        )?;
        let rows = stmt.query_map([track_id], |row| row.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn listening_stats(&self, since: Option<u64>, limit: usize) -> Result<ListeningStats> {
        let conn = self.conn.lock().unwrap();
        let since = since.unwrap_or(0) as i64;
        let limit = limit.max(1) as i64;

        let (plays, seconds, artists, albums, tracks, days) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(played_secs), 0), \
                    COUNT(DISTINCT NULLIF(artist, '')), \
                    COUNT(DISTINCT NULLIF(album, '')), \
                    COUNT(DISTINCT track_id), \
                    COUNT(DISTINCT date(started_at, 'unixepoch', 'localtime')) \
             FROM plays WHERE started_at >= ?1",
            [since],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?;

        Ok(ListeningStats {
            plays: plays.max(0) as u64,
            seconds: seconds.max(0) as u64,
            artists: artists.max(0) as u64,
            albums: albums.max(0) as u64,
            tracks: tracks.max(0) as u64,
            days: days.max(0) as u64,
            top_artists: ranked(&conn, RANKED_ARTISTS, since, limit)?,
            top_albums: ranked(&conn, RANKED_ALBUMS, since, limit)?,
            top_tracks: ranked(&conn, RANKED_TRACKS, since, limit)?,
        })
    }

    fn lyrics_variants(&self, track_id: i64) -> Result<Vec<StoredLyrics>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare_cached("SELECT source, text, not_found FROM lyrics WHERE track_id = ?1")?;
        let rows = stmt
            .query_map([track_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)?;
        let mut variants: Vec<StoredLyrics> = rows
            .into_iter()
            .filter_map(|(source, blob, not_found)| {
                if not_found {
                    return Some(StoredLyrics {
                        source,
                        text: String::new(),
                        not_found: true,
                    });
                }
                decompress_lyrics(&blob).map(|text| StoredLyrics {
                    source,
                    text,
                    not_found: false,
                })
            })
            .collect();
        variants.sort_by_key(|v| crate::lyrics_source::rank(&v.source, false));
        Ok(variants)
    }

    fn upsert_lyrics(
        &self,
        track_id: i64,
        text: &str,
        source: &str,
        not_found: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO lyrics (track_id, source, text, not_found, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(track_id, source) DO UPDATE SET \
                text = excluded.text, \
                not_found = excluded.not_found, \
                updated_at = excluded.updated_at",
            rusqlite::params![
                track_id,
                source,
                compress_lyrics(text),
                not_found as i64,
                unix_now()
            ],
        )?;
        Ok(())
    }

    fn track_artists_map(&self, track_ids: &[i64]) -> Result<HashMap<i64, Vec<String>>> {
        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = format!(
            "[{}]",
            track_ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "WITH ids(id) AS (SELECT value FROM json_each(?1)) \
             SELECT ta.track_id, a.name, ta.position FROM track_artists ta \
             JOIN artists a ON a.id = ta.artist_id \
             WHERE ta.track_id IN (SELECT id FROM ids) \
             UNION ALL \
             SELECT m.id, m.artist, 0 FROM media_items m \
             WHERE m.id IN (SELECT id FROM ids) AND m.artist <> '' \
             AND NOT EXISTS (SELECT 1 FROM track_artists x WHERE x.track_id = m.id) \
             ORDER BY 1, 3",
        )?;
        let rows = stmt.query_map([ids], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map: HashMap<i64, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, name) = row.map_err(LibraryError::Database)?;
            map.entry(id).or_default().push(name);
        }
        Ok(map)
    }

    fn artist_album_covers(&self, grouping: ArtistGrouping) -> Result<HashMap<i64, Vec<i64>>> {
        let conn = self.conn.lock().unwrap();
        let ctes = membership_ctes(grouping);
        let sql = format!(
            "WITH {ctes} \
             SELECT u.artist_id, al.cover_art_id, COALESCE(al.year, 9999999999) AS sort_year \
             FROM u \
             JOIN tracks t ON t.id = u.track_id \
             JOIN albums al ON al.id = t.album_id \
             WHERE al.cover_art_id IS NOT NULL \
             AND EXISTS (SELECT 1 FROM m WHERE m.artist_id = u.artist_id) \
             GROUP BY u.artist_id, al.id \
             ORDER BY u.artist_id, sort_year ASC, al.title COLLATE NOCASE"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in rows {
            let (artist_id, cover_art_id) = row.map_err(LibraryError::Database)?;
            let covers = map.entry(artist_id).or_default();
            if covers.len() < 3 && !covers.contains(&cover_art_id) {
                covers.push(cover_art_id);
            }
        }
        Ok(map)
    }

    fn cover_art_hashes(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT hash, id FROM cover_art")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn reconcile_local_sources(&self, folders: &[LocalFolder]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("UPDATE sources SET enabled = 0 WHERE kind = 'local'", [])?;
        for folder in folders {
            let name = Path::new(&folder.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| folder.path.clone());
            tx.execute(
                "INSERT INTO sources (kind, name, uri, enabled, available) \
                 VALUES ('local', ?1, ?2, 1, ?3) \
                 ON CONFLICT(kind, uri) DO UPDATE SET enabled = 1, available = excluded.available",
                rusqlite::params![name, folder.path, folder.available],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn has_media_under(&self, root: &str) -> Result<bool> {
        let separator = std::path::MAIN_SEPARATOR;
        let prefix = if root.ends_with(separator) {
            root.to_string()
        } else {
            format!("{root}{separator}")
        };
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM media_bindings b JOIN sources s ON s.id = b.source_id \
             WHERE s.kind = 'local' AND b.present = 1 \
             AND substr(b.source_key, 1, length(?1)) = ?1)",
            [prefix],
            |row| row.get(0),
        )?)
    }

    fn has_unplaced_media(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM media_bindings WHERE source_id = ?1 AND present = 1)",
            [PLACEHOLDER_SOURCE_ID],
            |row| row.get(0),
        )?)
    }

    fn sources(&self) -> Result<Vec<SourceSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.kind, s.uri, s.enabled, s.available, \
             (SELECT COUNT(*) FROM media_bindings b \
              WHERE b.source_id = s.id AND b.present = 1) \
             FROM sources s WHERE s.uri <> '' ORDER BY s.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SourceSummary {
                id: row.get(0)?,
                kind: row.get(1)?,
                uri: row.get(2)?,
                enabled: row.get(3)?,
                available: row.get(4)?,
                track_count: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn playback_locators(&self, item_id: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        // Only local folders can supply audio now, so the source_key is the
        // path and nothing needs to be composed. Cue pieces of one image come
        // back as several locators that share a path and differ by offset.
        let mut stmt = conn.prepare(
            "SELECT b.source_key, b.start_offset_ms \
             FROM media_bindings b JOIN sources s ON s.id = b.source_id \
             WHERE b.item_id = ?1 AND b.present = 1 AND s.enabled = 1 AND s.available = 1 \
             AND s.kind = 'local' \
             ORDER BY s.id, b.id",
        )?;
        let rows = stmt.query_map([item_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn invalidate_scan_fingerprint(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM scan_meta WHERE key = 'fingerprint'", [])?;
        Ok(())
    }

    fn refresh_item_snapshots(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(REFRESH_ITEM_SNAPSHOTS, [unix_now()])?;
        Ok(())
    }

    fn open_scan_session(&self) -> Result<Box<dyn ScanWrite>> {
        Ok(Box::new(ScanSession::open(&self.db_path)?))
    }

    fn scan_fingerprint(&self) -> Result<Option<String>> {
        self.scan_meta_value("fingerprint")
    }

    fn scan_folders(&self) -> Result<Option<String>> {
        self.scan_meta_value("folders")
    }

    fn set_scan_meta(&self, fingerprint: &str, folders: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO scan_meta (key, value) VALUES ('fingerprint', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [fingerprint],
        )?;
        tx.execute(
            "INSERT INTO scan_meta (key, value) VALUES ('folders', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [folders],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn vacuum(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("VACUUM; ANALYZE;")?;
        Ok(())
    }

    fn backups(&self) -> Vec<backup::BackupEntry> {
        backup::list_backups(&self.db_path)
    }

    fn request_restore(&self, backup: &Path) -> Result<()> {
        std::fs::write(restore_marker(&self.db_path), backup.to_string_lossy().as_bytes())?;
        Ok(())
    }

    fn integrity_report(&self) -> Result<IntegrityReport> {
        let conn = self.conn.lock().unwrap();
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap_or_else(|err| format!("could not run: {err}"));
        let foreign_key_violations: i64 =
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        let dangling_playlist_entries: i64 = conn.query_row(
            "SELECT COUNT(*) FROM playlist_tracks pt \
             WHERE NOT EXISTS (SELECT 1 FROM media_items m WHERE m.id = pt.track_id)",
            [],
            |row| row.get(0),
        )?;
        let orphaned_catalog_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM media_items m \
             WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.id = m.id) \
               AND NOT EXISTS (SELECT 1 FROM media_bindings b JOIN sources s ON s.id = b.source_id \
                               WHERE b.item_id = m.id AND b.present = 1 AND s.enabled = 1)",
            [],
            |row| row.get(0),
        )?;
        Ok(IntegrityReport {
            integrity,
            foreign_key_violations,
            dangling_playlist_entries,
            orphaned_catalog_rows,
        })
    }

    fn record_play(&self, play: &NewPlay) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let id: i64 = tx.query_row(
            "INSERT INTO plays (track_id, artist, title, album, album_artist, track_number, \
             duration_secs, played_secs, started_at, qualified) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(started_at, artist, title) DO UPDATE SET \
             played_secs = MAX(COALESCE(played_secs, 0), COALESCE(excluded.played_secs, 0)), \
             qualified = MAX(qualified, excluded.qualified) \
             RETURNING id",
            rusqlite::params![
                play.track_id,
                play.artist,
                play.title,
                play.album,
                play.album_artist,
                play.track_number.map(i64::from),
                play.duration_secs.map(|secs| secs as i64),
                play.played_secs.map(|secs| secs as i64),
                play.started_at as i64,
                play.qualified as i64,
            ],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(id)
    }

    fn play_tallies(&self, since: Option<u64>) -> Result<Vec<PlayTally>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT MAX(artist), MAX(title), MAX(album), COUNT(*), SUM(qualified), \
             MAX(started_at) FROM plays WHERE started_at >= ?1 \
             GROUP BY lower(artist), lower(title), lower(COALESCE(album, '')) \
             ORDER BY 5 DESC, 4 DESC, 6 DESC",
        )?;
        let since = since.map_or(0, |secs| secs as i64);
        let rows = stmt.query_map([since], |row| {
            Ok(PlayTally {
                artist: row.get(0)?,
                title: row.get(1)?,
                album: row.get::<_, Option<String>>(2)?.filter(|a| !a.is_empty()),
                plays: row.get::<_, i64>(3)? as u32,
                qualified_plays: row.get::<_, i64>(4)? as u32,
                last_played: row.get::<_, i64>(5)? as u64,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn recent_plays(&self, limit: usize) -> Result<Vec<RecentPlay>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT artist, title, album, started_at FROM plays \
             ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(RecentPlay {
                artist: row.get(0)?,
                title: row.get(1)?,
                album: row.get::<_, Option<String>>(2)?.filter(|a| !a.is_empty()),
                started_at: row.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }

    fn track_listings(&self) -> Result<Vec<TrackListing>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            r#"
            WITH stats AS (
                SELECT track_id, COUNT(*) AS plays, MAX(started_at) AS last_played
                FROM plays WHERE track_id IS NOT NULL GROUP BY track_id
            )
            SELECT
                t.id,
                t.title,
                COALESCE(
                    (SELECT a.name FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
                     WHERE ta.track_id = t.id ORDER BY ta.position LIMIT 1),
                    m.artist,
                    ''
                ),
                t.album_id,
                al.title,
                (SELECT a.name FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
                 WHERE aa.album_id = t.album_id ORDER BY aa.position LIMIT 1),
                COALESCE(t.year, al.year),
                COALESCE(s.plays, 0),
                s.last_played
            FROM tracks t
            LEFT JOIN media_items m ON m.id = t.id
            LEFT JOIN albums al ON al.id = t.album_id
            LEFT JOIN stats s ON s.track_id = t.id
            ORDER BY t.id
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TrackListing {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                album_id: row.get(3)?,
                album: row.get::<_, Option<String>>(4)?.filter(|a| !a.is_empty()),
                album_artist: row.get::<_, Option<String>>(5)?.filter(|a| !a.is_empty()),
                year: row.get(6)?,
                plays: row.get::<_, i64>(7)? as u32,
                last_played: row.get::<_, Option<i64>>(8)?.map(|secs| secs as u64),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::Database)
    }
}

impl SqliteLibrary {
    fn scan_meta_value(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let value = conn
            .query_row("SELECT value FROM scan_meta WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        Ok(value)
    }
}

/// Batched scan writer on a dedicated WAL connection. Holds one transaction
/// open across `SCAN_BATCH_SIZE` track inserts, then commits and reopens — so
/// the whole rescan costs a handful of `fsync`s instead of thousands. All
/// id resolution (artists / albums / covers) is served from in-memory caches,
/// eliminating the per-track `SELECT`s the old per-op path did.
pub struct ScanSession {
    conn: Connection,
    in_tx: bool,
    tx_started: std::time::Instant,
    finishing: bool,
    uncommitted: usize,
    artist_cache: HashMap<String, i64>,
    genre_cache: HashMap<String, i64>,
    album_cache: HashMap<(String, Option<i32>), i64>,
    stable_ids: Option<StableIds>,
    cover_cache: HashMap<String, i64>,
    pending_by_hash: HashMap<String, Vec<ScanTrack>>,
    roots: Vec<LocalRoot>,
    bindings: HashMap<(String, i64), (i64, i64)>,
    scan_id: i64,
    arrivals: Option<Vec<ScanTrack>>,
    written: HashMap<i64, i64>,
    placed: std::collections::HashSet<(i64, i64)>,
    seen: std::collections::HashSet<(String, i64)>,
}

impl ScanSession {
    fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        apply_pragmas(&conn)?;

        // Covers survive clear(), so seed the hash→id cache up front: most
        // covers on a re-scan resolve here with neither a thumbnail nor a SELECT.
        let mut cover_cache = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT hash, id FROM cover_art")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (hash, id) = row?;
                cover_cache.insert(hash, id);
            }
        }

        let roots = load_local_roots(&conn)?;
        let mut bindings = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT b.id, b.item_id, b.source_key, b.start_offset_ms FROM media_bindings b \
                 JOIN sources s ON s.id = b.source_id WHERE s.kind = 'local'",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    (row.get::<_, String>(2)?, row.get::<_, i64>(3)?),
                    (row.get::<_, i64>(0)?, row.get::<_, i64>(1)?),
                ))
            })?;
            for row in rows {
                let (key, value) = row?;
                bindings.insert(key, value);
            }
        }
        let scan_id: i64 = conn.query_row(
            "SELECT COALESCE(MAX(last_seen_scan), 0) + 1 FROM media_bindings",
            [],
            |row| row.get(0),
        )?;
        let has_items: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM media_items)", [], |row| {
                row.get(0)
            })?;
        let hold_arrivals = has_items || roots.len() > 1;

        let session = Self {
            conn,
            in_tx: false,
            tx_started: std::time::Instant::now(),
            finishing: false,
            uncommitted: 0,
            artist_cache: HashMap::new(),
            genre_cache: HashMap::new(),
            album_cache: HashMap::new(),
            stable_ids: None,
            cover_cache,
            pending_by_hash: HashMap::new(),
            roots,
            bindings,
            scan_id,
            arrivals: hold_arrivals.then(Vec::new),
            written: HashMap::new(),
            placed: std::collections::HashSet::new(),
            seen: std::collections::HashSet::new(),
        };
        Ok(session)
    }

    fn begin(&mut self) -> Result<()> {
        if !self.in_tx {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            self.in_tx = true;
            self.tx_started = std::time::Instant::now();
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        if self.in_tx {
            self.conn.execute_batch("COMMIT")?;
            self.in_tx = false;
            self.uncommitted = 0;
        }
        Ok(())
    }

    fn maybe_commit(&mut self) -> Result<()> {
        self.uncommitted += 1;
        if !self.finishing
            && (self.uncommitted >= SCAN_BATCH_SIZE || self.tx_started.elapsed() >= SCAN_BATCH_TIME)
        {
            self.commit()?;
        }
        Ok(())
    }

    fn resolve_artist(&mut self, name: &str) -> Result<i64> {
        let key = artist_key(name);
        if let Some(&id) = self.artist_cache.get(&key) {
            return Ok(id);
        }
        let sort_name = compute_sort_name(name);
        let wanted = match self
            .stable_ids
            .as_ref()
            .and_then(|ids| ids.artists.get(&key).copied())
        {
            Some(id) => id,
            None => allocate_catalog_id(&self.conn, "artists", ARTIST_ID_HIGH)?,
        };
        self.conn.execute(
            "INSERT INTO artists (id, name, sort_name) VALUES (?1, ?2, ?3)",
            rusqlite::params![wanted, name, sort_name],
        )?;
        let id = self.conn.last_insert_rowid();
        self.artist_cache.insert(key, id);
        Ok(id)
    }

    fn resolve_genre(&mut self, name: &str) -> Result<i64> {
        let key = name.to_lowercase();
        if let Some(&id) = self.genre_cache.get(&key) {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO genres (name, key) VALUES (?1, ?2)",
            [name, key.as_str()],
        )?;
        let id: i64 =
            self.conn
                .query_row("SELECT id FROM genres WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })?;
        self.genre_cache.insert(key, id);
        Ok(id)
    }

    /// The album row only; its cover is left NULL and filled afterwards by
    /// [`LibraryRepository::resolve_album_covers`], which picks deterministically
    /// instead of taking whichever track the scan happened to finish first.
    fn resolve_album(&mut self, title: &str, year: Option<i32>) -> Result<i64> {
        let key = (title.to_string(), year);
        if let Some(&id) = self.album_cache.get(&key) {
            return Ok(id);
        }
        let wanted = match self
            .stable_ids
            .as_ref()
            .and_then(|ids| ids.albums.get(&key).copied())
        {
            Some(id) => id,
            None => allocate_catalog_id(&self.conn, "albums", ALBUM_ID_HIGH)?,
        };
        self.conn.execute(
            "INSERT INTO albums (id, title, year, cover_art_id) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params![wanted, title, year],
        )?;
        let id = self.conn.last_insert_rowid();
        self.album_cache.insert(key, id);
        Ok(id)
    }

    fn start_offset(track: &ScanTrack) -> i64 {
        track.start_offset_ms.unwrap_or(0) as i64
    }

    fn title_of(track: &ScanTrack) -> String {
        track
            .title
            .clone()
            .unwrap_or_else(|| fallback_title_from_path(&track.path))
    }

    fn mint_item(&mut self, track: &ScanTrack) -> Result<i64> {
        let start_offset_ms = Self::start_offset(track);
        let cover_art_id = track
            .cover_hash
            .as_ref()
            .and_then(|h| self.cover_cache.get(h).copied());
        let title = Self::title_of(track);
        let (binding_id, item_id) = create_item(
            &self.conn,
            &self.spec(track),
            &ItemSnapshot {
                title: &title,
                artist: track.artist_names.first().map_or("", String::as_str),
                album: track.album_title.as_deref(),
                duration_ms: track.duration_ms.map(|n| n as i64),
                cover_art_id,
            },
        )?;
        self.bindings
            .insert((track.path.clone(), start_offset_ms), (binding_id, item_id));
        Ok(item_id)
    }

    fn adopt_item(&mut self, item_id: i64, track: &ScanTrack, tier: Tier) -> Result<()> {
        let start_offset_ms = Self::start_offset(track);
        let binding_id = bind_item(&self.conn, item_id, &self.spec(track))?;
        self.conn.execute(
            "INSERT INTO adoptions (item_id, binding_id, tier, at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![item_id, binding_id, tier.as_str(), unix_now()],
        )?;
        self.bindings
            .insert((track.path.clone(), start_offset_ms), (binding_id, item_id));
        Ok(())
    }

    fn spec<'a>(&self, track: &'a ScanTrack) -> BindingSpec<'a> {
        BindingSpec {
            source_id: source_for_path(&self.roots, &track.path),
            key: &track.path,
            start_offset_ms: Self::start_offset(track),
            file_size: track.file_size.map(|size| size as i64),
            scan_id: self.scan_id,
        }
    }

    fn descriptor_of(&self, track: &ScanTrack) -> Descriptor {
        Descriptor {
            title: Self::title_of(track),
            artist: track.artist_names.first().cloned().unwrap_or_default(),
            artist_aliases: track.artist_names.iter().skip(1).cloned().collect(),
            album: track.album_title.clone(),
            duration_ms: track.duration_ms.map(|n| n as i64),
            files: track
                .file_size
                .map(|size| {
                    (
                        size as i64,
                        file_piece(track.is_cue, Self::start_offset(track)),
                    )
                })
                .into_iter()
                .collect(),
            sources: vec![source_for_path(&self.roots, &track.path)],
            ..Default::default()
        }
    }

    fn place(&mut self, track: ScanTrack, item_id: i64) -> Result<()> {
        let source_id = source_for_path(&self.roots, &track.path);
        self.placed.insert((item_id, source_id));
        match self.written.get(&item_id) {
            None => self.write_track(track, item_id),
            Some(&written_by) if source_id < written_by => {
                for sql in UNWRITE_TRACK {
                    self.conn.execute(sql, [item_id])?;
                }
                self.write_track(track, item_id)
            }
            Some(_) => self.maybe_commit(),
        }
    }

    fn absorb_item(&mut self, live: i64, orphan: i64, tier: Tier) -> Result<()> {
        self.conn.pragma_update(None, "defer_foreign_keys", "ON")?;
        let adopted: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM media_bindings WHERE item_id = ?1")?;
            stmt.query_map([live], |row| row.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for sql in ABSORB_ITEM {
            self.conn.execute(sql, [orphan, live])?;
        }
        let now = unix_now();
        for binding_id in adopted {
            self.conn.execute(
                "INSERT INTO adoptions (item_id, binding_id, tier, at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![orphan, binding_id, tier.as_str(), now],
            )?;
        }
        if let Some(source_id) = self.written.remove(&live) {
            self.written.insert(orphan, source_id);
        }
        Ok(())
    }

    fn retire_unplaced_bindings(&mut self) -> Result<()> {
        let unplaced: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, source_key FROM media_bindings WHERE source_id = ?1 AND present = 1",
            )?;
            stmt.query_map([PLACEHOLDER_SOURCE_ID], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?
        };
        for (binding_id, key) in unplaced {
            if root_for_path(&self.roots, &key).is_none_or(|root| root.available) {
                self.conn.execute(
                    "UPDATE media_bindings SET present = 0 WHERE id = ?1",
                    [binding_id],
                )?;
            }
        }
        Ok(())
    }

    fn revive_lost_items(&mut self) -> Result<()> {
        let identities = load_identities(&self.conn)?;
        let occupies = |b: &BindingFacts| b.enabled && b.present;
        let lost: Vec<Descriptor> = identities
            .iter()
            .filter(|identity| identity.cherished && !identity.playable())
            .map(|identity| identity.descriptor(&occupies))
            .collect();
        if lost.is_empty() {
            return Ok(());
        }
        let spare: Vec<Descriptor> = identities
            .iter()
            .filter(|identity| !identity.cherished && identity.playable())
            .map(|identity| identity.descriptor(&occupies))
            .collect();
        let assignments = match_tracks(&spare, &lost);
        let mut revived = 0usize;
        for (live, assignment) in spare.iter().zip(assignments) {
            if let Some((orphan, tier)) = assignment {
                self.absorb_item(live.item_id, orphan, tier)?;
                revived += 1;
            }
        }
        if revived > 0 {
            log::info!("Re-attached {revived} files to entries that had lost theirs");
        }
        Ok(())
    }

    fn settle_arrivals(&mut self, mut tracks: Vec<ScanTrack>) -> Result<()> {
        tracks.sort_by(|a, b| {
            (a.path.as_str(), Self::start_offset(a)).cmp(&(b.path.as_str(), Self::start_offset(b)))
        });
        tracks.dedup_by(|a, b| a.path == b.path && Self::start_offset(a) == Self::start_offset(b));
        let mut groups: std::collections::BTreeMap<i64, Vec<ScanTrack>> = Default::default();
        for track in tracks {
            groups
                .entry(source_for_path(&self.roots, &track.path))
                .or_default()
                .push(track);
        }
        let mut adopted = 0usize;
        for (_, group) in groups {
            let scan_id = self.scan_id;
            let occupies = |b: &BindingFacts| {
                b.enabled
                    && if b.local && b.available {
                        b.last_seen_scan == scan_id
                    } else {
                        b.present
                    }
            };
            let candidates: Vec<Descriptor> = load_identities(&self.conn)?
                .iter()
                .map(|identity| identity.descriptor(&occupies))
                .collect();
            let arrivals: Vec<Descriptor> = group
                .iter()
                .map(|track| self.descriptor_of(track))
                .collect();
            let assignments = match_tracks(&arrivals, &candidates);
            for (track, assignment) in group.into_iter().zip(assignments) {
                let path = track.path.clone();
                let settled = match assignment {
                    Some((item_id, tier)) => self.adopt_item(item_id, &track, tier).map(|()| {
                        adopted += 1;
                        item_id
                    }),
                    None => self.mint_item(&track),
                }
                .and_then(|item_id| self.place(track, item_id));
                if let Err(e) = settled {
                    log::error!("Failed to insert track {path}: {e}");
                }
            }
        }
        if adopted > 0 {
            log::info!("Re-attached {adopted} moved or renamed files to their library entries");
        }
        Ok(())
    }

    fn insert_track(&mut self, track: ScanTrack) -> Result<()> {
        let start_offset_ms = Self::start_offset(&track);
        let key = (track.path.clone(), start_offset_ms);
        if !self.seen.insert(key.clone()) {
            return Ok(());
        }
        let source_id = source_for_path(&self.roots, &track.path);
        if !self.bindings.contains_key(&key)
            && let Some(arrivals) = &mut self.arrivals
        {
            arrivals.push(track);
            return Ok(());
        }
        self.begin()?;
        let item_id = if let Some(&(binding_id, item_id)) = self.bindings.get(&key)
            && self.placed.contains(&(item_id, source_id))
        {
            self.conn
                .execute("DELETE FROM media_bindings WHERE id = ?1", [binding_id])?;
            self.bindings.remove(&key);
            self.mint_item(&track)?
        } else if let Some(&(binding_id, item_id)) = self.bindings.get(&key) {
            self.conn.execute(
                "UPDATE media_bindings SET present = 1, last_seen_scan = ?1, last_seen_at = ?2, \
                 source_id = ?3, file_size = ?4 WHERE id = ?5",
                rusqlite::params![
                    self.scan_id,
                    unix_now(),
                    source_id,
                    track.file_size.map(|size| size as i64),
                    binding_id
                ],
            )?;
            item_id
        } else {
            self.mint_item(&track)?
        };
        self.place(track, item_id)
    }

    fn write_track(&mut self, track: ScanTrack, item_id: i64) -> Result<()> {
        self.written
            .insert(item_id, source_for_path(&self.roots, &track.path));
        let cover_id = track
            .cover_hash
            .as_ref()
            .and_then(|h| self.cover_cache.get(h).copied());

        let mut artist_ids = Vec::with_capacity(track.artist_names.len());
        for (pos, name) in track.artist_names.iter().enumerate() {
            let id = self.resolve_artist(name)?;
            artist_ids.push((id, pos as i64));
        }

        let mut album_artist_ids = Vec::with_capacity(track.album_artist_names.len());
        for (pos, name) in track.album_artist_names.iter().enumerate() {
            let id = self.resolve_artist(name)?;
            album_artist_ids.push((id, pos as i64));
        }

        let mut genre_ids = Vec::with_capacity(track.genres.len());
        for name in &track.genres {
            genre_ids.push(self.resolve_genre(name)?);
        }

        let album_id = match &track.album_title {
            Some(title) => Some(self.resolve_album(title, track.year)?),
            None => None,
        };

        let title = Self::title_of(&track);
        let track_number = track.track_number.map(|n| n as i64);
        let disc_number = track.disc_number.unwrap_or(1) as i64;
        let duration_ms = track.duration_ms.map(|n| n as i64);
        let start_offset_ms = Self::start_offset(&track);

        // OR IGNORE: the same file can appear under two overlapping scan roots.
        // The UNIQUE(path, start_offset_ms) index drops the duplicate instead of
        // failing the statement.
        let inserted = self.conn.execute(
            r#"INSERT OR IGNORE INTO tracks
                (id, path, title, album_id, track_number, disc_number, duration_ms, year, cover_art_id, start_offset_ms, bitrate, is_cue)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
            rusqlite::params![
                item_id,
                track.path,
                title,
                album_id,
                track_number,
                disc_number,
                duration_ms,
                track.year,
                cover_id,
                start_offset_ms,
                track.bitrate,
                track.is_cue,
            ],
        )?;
        // A duplicate was ignored: bail before linking artists, otherwise
        // last_insert_rowid() would still point at the prior insert and we'd
        // attach track_artists rows to the wrong track.
        if inserted == 0 {
            return self.maybe_commit();
        }
        let track_id = item_id;

        if !track.replaygain.is_empty() {
            self.conn.execute(
                "INSERT OR REPLACE INTO track_gain \
                 (track_id, track_gain, track_peak, album_gain, album_peak) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    track_id,
                    track.replaygain.track_gain.map(f64::from),
                    track.replaygain.track_peak.map(f64::from),
                    track.replaygain.album_gain.map(f64::from),
                    track.replaygain.album_peak.map(f64::from),
                ],
            )?;
        }

        for (artist_id, position) in &artist_ids {
            self.conn.execute(
                "INSERT INTO track_artists (track_id, artist_id, role, position) VALUES (?1, ?2, 'main', ?3)",
                [track_id, *artist_id, *position],
            )?;
        }

        for (artist_id, position) in &album_artist_ids {
            self.conn.execute(
                "INSERT OR IGNORE INTO track_album_artists (track_id, artist_id, position) VALUES (?1, ?2, ?3)",
                [track_id, *artist_id, *position],
            )?;
        }

        for (position, genre_id) in genre_ids.iter().enumerate() {
            self.conn.execute(
                "INSERT OR IGNORE INTO track_genres (track_id, genre_id, position) VALUES (?1, ?2, ?3)",
                [track_id, *genre_id, position as i64],
            )?;
        }

        for lyrics in &track.lyrics {
            self.conn.execute(
                "INSERT INTO lyrics (track_id, source, text, not_found, updated_at) \
                 VALUES (?1, ?2, ?3, 0, ?4) \
                 ON CONFLICT(track_id, source) DO UPDATE SET \
                 text = excluded.text, not_found = 0, updated_at = excluded.updated_at",
                rusqlite::params![
                    track_id,
                    lyrics.source,
                    compress_lyrics(&lyrics.text),
                    unix_now(),
                ],
            )?;
        }

        self.maybe_commit()
    }
}

impl ScanWrite for ScanSession {
    fn flush(&mut self) -> Result<()> {
        self.commit()
    }

    fn clear(&mut self) -> Result<()> {
        self.begin()?;
        self.stable_ids = Some(StableIds::load(&self.conn)?);
        self.conn.execute_batch(CLEAR_CATALOG)?;
        Ok(())
    }

    fn add_cover(
        &mut self,
        hash: &str,
        small: Vec<u8>,
        large: Vec<u8>,
        source_path: &str,
        embedded: bool,
    ) -> Result<()> {
        if !self.cover_cache.contains_key(hash) {
            self.begin()?;
            self.conn.execute(
                "INSERT OR IGNORE INTO cover_art (hash, small, large, source_path, embedded) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![hash, small, large, source_path, embedded],
            )?;
            let id: i64 =
                self.conn
                    .query_row("SELECT id FROM cover_art WHERE hash = ?1", [hash], |row| {
                        row.get(0)
                    })?;
            self.cover_cache.insert(hash.to_string(), id);
            self.maybe_commit()?;
        }
        if let Some(tracks) = self.pending_by_hash.remove(hash) {
            for track in tracks {
                self.insert_track(track)?;
            }
        }
        Ok(())
    }

    fn add_track(&mut self, track: ScanTrack) -> Result<()> {
        if let Some(hash) = &track.cover_hash
            && !self.cover_cache.contains_key(hash)
        {
            // Cover thumbnail hasn't been inserted yet; hold the track until the
            // matching add_cover arrives (it always does for a claimed hash).
            self.pending_by_hash
                .entry(hash.clone())
                .or_default()
                .push(track);
            return Ok(());
        }
        self.insert_track(track)
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        self.begin()?;
        self.finishing = true;
        // Any track still waiting on a cover that never materialized (e.g. a
        // thumbnail-generation error) is inserted cover-less.
        let leftovers: Vec<ScanTrack> = self
            .pending_by_hash
            .drain()
            .flat_map(|(_, tracks)| tracks)
            .map(|mut t| {
                t.cover_hash = None;
                t
            })
            .collect();
        for track in leftovers {
            self.insert_track(track)?;
        }
        if let Some(arrivals) = self.arrivals.take()
            && !arrivals.is_empty()
        {
            self.settle_arrivals(arrivals)?;
        }
        self.conn
            .execute(RETIRE_UNSEEN_LOCAL_BINDINGS, [self.scan_id])?;
        self.retire_unplaced_bindings()?;
        self.revive_lost_items()?;
        self.conn.execute(SWEEP_UNREFERENCED_ITEMS, [])?;
        self.commit()
    }
}

fn compute_sort_name(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_lowercase();
    if let Some(rest) = lower.strip_prefix("the ") {
        return format!("{}, the", rest);
    }
    if let Some(rest) = lower.strip_prefix("a ") {
        return format!("{}, a", rest);
    }
    trimmed.to_string()
}

fn fallback_title_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

fn compute_sha256(data: &[u8]) -> String {
    sha256_hex(data)
}

/// Hex-encoded SHA-256 of `data`. Used both for cover-art content addressing
/// and for the scan fingerprint, so the indexer can dedupe covers with the
/// exact same hash the DB stores.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(data);
    format!("{:x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_sort_name_the() {
        assert_eq!(compute_sort_name("The Beatles"), "beatles, the");
    }

    #[test]
    fn test_lyrics_compression_round_trips_unicode() {
        let text = "[00:01.00]Привет, мир\n[00:02.00]こんにちは\n[00:03.00]🎵";
        assert_eq!(
            decompress_lyrics(&compress_lyrics(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn test_lyrics_compression_shrinks_repetitive_text() {
        let chorus = "[00:30.00]We are the champions, my friend\n".repeat(40);
        let compressed = compress_lyrics(&chorus);
        assert!(compressed.len() * 3 < chorus.len());
        assert_eq!(
            decompress_lyrics(&compressed).as_deref(),
            Some(chorus.as_str())
        );
    }

    #[test]
    fn test_decompress_garbage_is_none() {
        assert!(decompress_lyrics(b"not a zlib stream").is_none());
    }

    #[test]
    fn test_compute_sort_name_a() {
        assert_eq!(
            compute_sort_name("A Tribe Called Quest"),
            "tribe called quest, a"
        );
    }

    #[test]
    fn test_compute_sort_name_no_article() {
        assert_eq!(compute_sort_name("Radiohead"), "Radiohead");
    }

    #[test]
    fn test_compute_sort_name_whitespace() {
        assert_eq!(compute_sort_name("  The Who  "), "who, the");
    }

    #[test]
    fn test_compute_sort_name_lowercase_article() {
        assert_eq!(compute_sort_name("the national"), "national, the");
    }

    #[test]
    fn test_compute_sort_name_empty() {
        assert_eq!(compute_sort_name(""), "");
    }

    #[test]
    fn test_fallback_title_from_path_basic() {
        assert_eq!(fallback_title_from_path("/music/song.flac"), "song");
    }

    #[test]
    fn test_fallback_title_from_path_no_ext() {
        assert_eq!(fallback_title_from_path("/music/song"), "song");
    }

    #[test]
    fn test_fallback_title_from_path_root() {
        assert_eq!(fallback_title_from_path("song.flac"), "song");
    }

    #[test]
    fn test_fallback_title_from_path_multiple_ext() {
        assert_eq!(fallback_title_from_path("/music/song.tar.gz"), "song.tar");
    }

    #[test]
    fn test_fallback_title_from_path_empty() {
        assert_eq!(fallback_title_from_path(""), "");
    }

    fn open_test_lib() -> SqliteLibrary {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let temp_dir = std::env::temp_dir().join("pulse-music-library-sqlite");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join(format!(
            "test-{}-{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&db_path);
        SqliteLibrary::open_at(&db_path).unwrap()
    }

    #[test]
    fn test_upsert_lyrics_keeps_one_row_per_source() {
        let lib = open_test_lib();
        let track_id = lib
            .upsert_track(
                &NewTrack {
                    path: "/m/u.flac".into(),
                    title: Some("U".into()),
                    ..Default::default()
                },
                None,
                &[],
            )
            .unwrap();
        let find = |source: &str| {
            lib.lyrics_variants(track_id)
                .unwrap()
                .into_iter()
                .find(|v| v.source == source)
                .unwrap()
        };

        lib.upsert_lyrics(track_id, "plain words", "embedded", false)
            .unwrap();
        let first = find("embedded");
        assert!(!first.not_found);
        assert_eq!(first.text, "plain words");

        lib.upsert_lyrics(track_id, "[00:00.00] synced now", "lrclib", false)
            .unwrap();
        assert_eq!(find("lrclib").text, "[00:00.00] synced now");
        assert_eq!(find("embedded").text, "plain words");

        lib.upsert_lyrics(track_id, "", "lrclib", true).unwrap();
        let third = find("lrclib");
        assert!(third.not_found);
        assert_eq!(third.text, "");

        let count: i64 = {
            let conn = lib.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM lyrics WHERE track_id = ?1",
                [track_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(count, 2);
    }

    fn add_track(lib: &SqliteLibrary, path: &str) {
        lib.upsert_track(
            &NewTrack {
                path: path.into(),
                title: None,
                ..Default::default()
            },
            None,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn child_folders_group_by_immediate_subdirectory() {
        let lib = open_test_lib();
        for path in [
            "/music/Rock/Nightwish/a.flac",
            "/music/Rock/Nightwish/b.flac",
            "/music/Rock/Metallica/c.flac",
            "/music/Rock/loose.flac",
            "/music/Jazz/d.flac",
        ] {
            add_track(&lib, path);
        }

        let names: Vec<String> = lib
            .child_folders("/music")
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        // Sorted case-insensitively; "loose.flac" is a file, not a folder.
        assert_eq!(names, vec!["Jazz".to_string(), "Rock".to_string()]);

        let rock = lib
            .child_folders("/music")
            .unwrap()
            .into_iter()
            .find(|f| f.name == "Rock")
            .unwrap();
        // Counted at any depth: both Nightwish tracks, the Metallica track and
        // the loose file that sits directly in Rock/.
        assert_eq!(rock.track_count, 4);

        let nightwish = lib.child_folders("/music/Rock").unwrap();
        assert_eq!(nightwish.len(), 2);
        assert_eq!(nightwish[0].name, "Metallica");
        assert_eq!(nightwish[1].name, "Nightwish");
        assert_eq!(nightwish[1].track_count, 2);
        assert_eq!(nightwish[1].path, "/music/Rock/Nightwish");
    }

    /// Every level orders by name ignoring case, because the roots list does.
    /// Raw byte order put "Zebra" above "apple" on every level below the top.
    #[test]
    fn child_folders_sort_ignoring_case() {
        let lib = open_test_lib();
        for path in [
            "/music/Zebra/a.flac",
            "/music/apple/b.flac",
            "/music/Mango/c.flac",
        ] {
            add_track(&lib, path);
        }

        let names: Vec<String> = lib
            .child_folders("/music")
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();

        assert_eq!(
            names,
            vec!["apple".to_string(), "Mango".to_string(), "Zebra".to_string()]
        );
    }

    /// "No album" is not an album. Counting the `NULL` group claimed one album
    /// for a folder that holds only loose, untagged files.
    #[test]
    fn child_folders_do_not_count_untagged_tracks_as_an_album() {
        let lib = open_test_lib();
        add_track(&lib, "/music/Loose/a.flac");
        add_track(&lib, "/music/Loose/b.flac");
        let album = lib.upsert_album("Real", Some(2020), None).unwrap();
        lib.upsert_track(
            &NewTrack {
                path: "/music/Loose/c.flac".into(),
                title: None,
                ..Default::default()
            },
            Some(album),
            &[],
        )
        .unwrap();

        let loose = lib
            .child_folders("/music")
            .unwrap()
            .into_iter()
            .find(|f| f.name == "Loose")
            .unwrap();

        assert_eq!(loose.track_count, 3);
        assert_eq!(loose.album_count, 1);
    }

    /// The "tracks in this folder" row counts the files that sit directly in the
    /// folder, not the whole subtree — otherwise it would duplicate the total
    /// already shown on the folder's own row.
    #[test]
    fn direct_track_count_ignores_subdirectories() {
        let lib = open_test_lib();
        for path in [
            "/music/Rock/loose.flac",
            "/music/Rock/also.flac",
            "/music/Rock/Nightwish/a.flac",
            "/music/Rock/Nightwish/b.flac",
            "/music/Jazz/d.flac",
        ] {
            add_track(&lib, path);
        }

        assert_eq!(lib.direct_track_count("/music/Rock").unwrap(), 2);
        assert_eq!(lib.direct_track_count("/music/Rock/Nightwish").unwrap(), 2);
        assert_eq!(lib.direct_track_count("/music").unwrap(), 0);
        assert_eq!(lib.direct_track_count("/music/Nothing").unwrap(), 0);
    }

    /// The row this backs must show the folder's own files, not the subtree —
    /// the subtree is already reachable through the subdirectory rows.
    #[test]
    fn tracks_directly_in_folder_skips_subdirectories() {
        let lib = open_test_lib();
        for path in [
            "/music/Rock/b.flac",
            "/music/Rock/a.flac",
            "/music/Rock/Nightwish/deep.flac",
        ] {
            add_track(&lib, path);
        }

        let direct: Vec<String> = lib
            .tracks_directly_in_folder("/music/Rock")
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();

        assert_eq!(
            direct,
            vec!["/music/Rock/a.flac".to_string(), "/music/Rock/b.flac".to_string()]
        );
        // The subtree query is untouched and still sees everything.
        assert_eq!(lib.tracks_in_folder("/music/Rock").unwrap().len(), 3);
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_a_child() {
        let lib = open_test_lib();
        add_track(&lib, "/music/Rocknroll/Par/a.flac");
        add_track(&lib, "/music/Rock/Tool/b.flac");

        // "/music/Rock" must not swallow "/music/Rocknroll".
        let names: Vec<String> = lib
            .child_folders("/music")
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, vec!["Rock".to_string(), "Rocknroll".to_string()]);
    }

    #[test]
    fn like_wildcards_in_a_folder_name_are_matched_literally() {
        let lib = open_test_lib();
        add_track(&lib, "/music/100%_Hits/a.flac");
        add_track(&lib, "/music/1000XHits/b.flac");

        let found = lib.child_folders("/music/100%_Hits").unwrap();
        assert_eq!(found.len(), 0, "the folder itself has no subfolders");

        let tracks = lib.tracks_in_folder("/music/100%_Hits").unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].path, "/music/100%_Hits/a.flac");
    }

    #[test]
    fn tracks_in_folder_covers_everything_below_the_path() {
        let lib = open_test_lib();
        add_track(&lib, "/music/Jazz/A/a.flac");
        add_track(&lib, "/music/Jazz/B/b.flac");
        add_track(&lib, "/music/Rock/c.flac");

        let mut paths: Vec<String> = lib
            .tracks_in_folder("/music/Jazz")
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "/music/Jazz/A/a.flac".to_string(),
                "/music/Jazz/B/b.flac".to_string()
            ]
        );
    }

    #[test]
    fn folder_roots_skips_roots_with_no_tracks_and_tolerates_trailing_slashes() {
        let lib = open_test_lib();
        lib.reconcile_local_sources(&[
            LocalFolder {
                path: "/music/".into(),
                available: true,
            },
            LocalFolder {
                path: "/empty".into(),
                available: true,
            },
        ])
        .unwrap();
        add_track(&lib, "/music/Rock/a.flac");

        let roots = lib.folder_roots().unwrap();
        assert_eq!(roots.len(), 1, "the empty root must not be listed");
        assert_eq!(roots[0].name, "music");
        // The trailing slash is normalised away, so children still resolve.
        assert_eq!(roots[0].path, "/music");
        assert_eq!(roots[0].track_count, 1);
    }

    /// Reads back every track path matching `dir` and everything below it,
    /// exactly the way the folder queries do.
    fn paths_matching(conn: &rusqlite::Connection, dir: &str, separator: char) -> Vec<String> {
        conn.prepare(r"SELECT path FROM tracks WHERE path LIKE ?1 ESCAPE '\'")
            .unwrap()
            .query_map([folder_like_pattern_with(dir, separator)], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    /// The Windows half of the folder hierarchy, exercised from any host.
    ///
    /// Track paths are stored exactly as the indexer saw them, so on Windows
    /// they carry a `\`. A pattern or a join that hardcoded `/` matched
    /// nothing there, `folder_roots` then dropped every root with a track
    /// count of zero, and the Folders tab came up empty instead of listing
    /// anything.
    #[test]
    fn a_windows_shaped_folder_query_matches_windows_shaped_track_paths() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tracks (path TEXT NOT NULL)")
            .unwrap();
        for path in [r"D:\music\Rock\a.flac", r"D:\music\Jazz\b.flac"] {
            conn.execute("INSERT INTO tracks VALUES (?1)", [path])
                .unwrap();
        }

        let matched = paths_matching(&conn, r"D:\music", '\\');

        assert_eq!(matched.len(), 2, "the pattern matched nothing: {matched:?}");
    }

    /// The trailing separator is what stops `.../Rock` from also matching a
    /// sibling called `.../Rocknroll`, and it has to be the host's.
    #[test]
    fn folder_like_pattern_only_reaches_below_the_directory() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tracks (path TEXT NOT NULL)")
            .unwrap();
        for path in ["/music/Rock/a.flac", "/music/Rocknroll/b.flac"] {
            conn.execute("INSERT INTO tracks VALUES (?1)", [path])
                .unwrap();
        }

        assert_eq!(
            paths_matching(&conn, "/music/Rock", '/'),
            vec!["/music/Rock/a.flac".to_string()]
        );
    }

    /// A drive root is one separator wider than its designator, so joining a
    /// folder onto `D:` would produce the drive-relative `D:Rock`.
    #[test]
    fn clean_dir_keeps_the_separator_on_a_drive_root() {
        if std::path::MAIN_SEPARATOR == '/' {
            assert_eq!(clean_dir("/"), "/");
            assert_eq!(clean_dir("/music/"), "/music");
            assert_eq!(clean_dir("/music///"), "/music");
        } else {
            assert_eq!(clean_dir("D:\\"), "D:\\");
            assert_eq!(clean_dir("D:\\music\\"), "D:\\music");
        }
    }

    /// The separator inside a directory name has to survive as itself, or the
    /// pattern matches a different directory than the one asked for.
    #[test]
    fn folder_like_pattern_escapes_a_separator_inside_a_directory_name() {
        assert_eq!(
            folder_like_pattern_with(r"C:\my\music", '\\'),
            "C:\\\\my\\\\music\\\\%"
        );
        assert_eq!(folder_like_pattern_with("/my/music", '/'), "/my/music/%");
        assert_eq!(
            folder_like_pattern_with("/100%_pure", '/'),
            "/100\\%\\_pure/%"
        );
    }

    /// A root already ends in its separator, so the pattern must not add a
    /// second one: `//%` demands a path with an empty leading component and
    /// matches nothing at all, which made a library root of `/` show zero
    /// tracks.
    #[test]
    fn folder_like_pattern_does_not_double_a_root_separator() {
        assert_eq!(folder_like_pattern_with("/", '/'), "/%");
        assert_eq!(folder_like_pattern_with("D:\\", '\\'), "D:\\\\%");

        // And they still match what they are supposed to.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tracks (path TEXT NOT NULL)")
            .unwrap();
        for path in ["/music/a.flac", "/other/b.flac"] {
            conn.execute("INSERT INTO tracks VALUES (?1)", [path])
                .unwrap();
        }
        assert_eq!(paths_matching(&conn, "/", '/').len(), 2);
    }
}
