use std::collections::HashMap;

use crate::error::Result;
use crate::models::{
    AlbumSearchEntry, AlbumSummary, ArtistGrouping, ArtistSummary, CoverArt,
    FolderSummary, LocalFolder, NewPlay, NewTrack,
    PlayTally, PlaylistSummary, RecentPlay, ScanTrack, SourceSummary,
    StoredLyrics, Track, TrackListing, ListeningStats,
};

/// A batched, single-transaction sink for a full rescan. Implementations own a
/// dedicated DB connection (separate from the repository's read connection) so
/// the UI can keep reading while a scan writes. Covers and tracks arrive in any
/// order; a track whose cover hash has not been seen yet is buffered until the
/// matching [`add_cover`](ScanWrite::add_cover) (or inserted cover-less at
/// [`finish`](ScanWrite::finish)).
pub trait ScanWrite: Send {
    fn clear(&mut self) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
    /// Register a freshly generated cover thumbnail by content hash.
    fn add_cover(
        &mut self,
        hash: &str,
        small: Vec<u8>,
        large: Vec<u8>,
        source_path: &str,
        embedded: bool,
    ) -> Result<()>;
    /// Insert one scanned track, resolving artists/album/cover from caches.
    fn add_track(&mut self, track: ScanTrack) -> Result<()>;
    /// Flush any buffered tracks and commit.
    fn finish(self: Box<Self>) -> Result<()>;
}

pub trait LibraryRepository: Send + Sync {
    fn upsert_artist(&self, name: &str) -> Result<i64>;
    fn upsert_album(
        &self,
        title: &str,
        year: Option<i32>,
        cover_art_id: Option<i64>,
    ) -> Result<i64>;
    fn set_album_artists(&self, album_id: i64, artist_ids: &[(i64, i32)]) -> Result<()>;
    fn set_track_album_artists(&self, track_id: i64, artist_ids: &[(i64, i32)]) -> Result<()>;
    fn track_album_artists(&self, track_id: i64) -> Result<Vec<String>>;
    fn upsert_track(
        &self,
        track: &NewTrack,
        album_id: Option<i64>,
        artist_ids: &[(i64, i32)],
    ) -> Result<i64>;
    fn albums(&self) -> Result<Vec<AlbumSummary>>;
    fn album_search_entries(&self) -> Result<Vec<AlbumSearchEntry>>;
    fn tracks_for_album(&self, album_id: i64) -> Result<Vec<Track>>;
    fn album_track_counts(&self) -> Result<HashMap<i64, i64>>;
    fn track_artists(&self, track_id: i64) -> Result<Vec<String>>;
    fn track_artists_with_ids(&self, track_id: i64) -> Result<Vec<(i64, String)>>;
    fn track_artists_map(&self, track_ids: &[i64]) -> Result<HashMap<i64, Vec<String>>>;
    fn album_title(&self, album_id: i64) -> Result<Option<String>>;
    fn album_genres(&self, album_id: i64) -> Result<Vec<String>>;
    /// The album's credited artists in `album_artists.position` order. Mirrors
    /// [`album_genres`](LibraryRepository::album_genres); needed where an album's
    /// artists must be shown without walking every album summary.
    fn album_artists(&self, album_id: i64) -> Result<Vec<String>>;
    fn album_genres_map(&self) -> Result<HashMap<i64, Vec<String>>>;
    fn clear(&self) -> Result<()>;
    fn has_tracks(&self) -> Result<bool>;
    fn delete_orphaned_albums_and_artists(&self) -> Result<()>;
    /// Store a cover (deduplicated by content hash) along with where it came from, so
    /// the "open the original" paths can reach the full-size image later. Mirrors what
    /// [`ScanWrite::add_cover`] records, except the thumbnails are generated here —
    /// the point-update path has no pipeline to make them.
    fn save_cover_art(&self, data: &[u8], source_path: &str, embedded: bool) -> Result<i64>;
    /// Set or clear one track's cover. Needed because [`LibraryRepository::upsert_track`]
    /// `COALESCE`s the column, so passing `None` there keeps the old value instead of
    /// clearing it.
    fn set_track_cover(&self, track_id: i64, cover_art_id: Option<i64>) -> Result<()>;
    fn get_cover_art(&self, id: i64) -> Result<Option<CoverArt>>;
    fn get_cover_art_small(&self, id: i64) -> Result<Option<Vec<u8>>>;
    fn get_cover_art_large(&self, id: i64) -> Result<Option<Vec<u8>>>;
    fn get_cover_art_source(&self, id: i64) -> Result<Option<(String, bool)>>;
    fn get_track_path_for_cover(&self, id: i64) -> Result<Option<String>>;
    /// Point every album at the cover of its lowest-numbered track that carries one,
    /// and at nothing when none does. The order is fully specified, so a scan and a
    /// point update land on the same row no matter which track either happened to
    /// process first — without that, an album whose tracks hold different art would
    /// change its cover between rescans.
    fn resolve_album_covers(&self) -> Result<()>;
    fn resolve_album_artists(&self) -> Result<()>;
    fn album_artist_known(&self, album_id: i64) -> Result<bool>;
    fn artists(&self, grouping: ArtistGrouping) -> Result<Vec<ArtistSummary>>;
    fn artist_summary(&self, id: i64, grouping: ArtistGrouping) -> Result<Option<ArtistSummary>>;
    fn listed_artist_for_credit(
        &self,
        track_id: i64,
        credited_artist_id: i64,
        grouping: ArtistGrouping,
    ) -> Result<Option<i64>>;
    fn artist_name(&self, id: i64) -> Result<Option<String>>;
    fn artist_search_haystacks(&self, grouping: ArtistGrouping) -> Result<HashMap<i64, String>>;
    fn artist_album_covers(&self, grouping: ArtistGrouping) -> Result<HashMap<i64, Vec<i64>>>;
    fn tracks_by_artist(&self, artist_id: i64, grouping: ArtistGrouping) -> Result<Vec<Track>>;
    /// Configured local roots that contain at least one track. These are the
    /// top level of the folder hierarchy.
    fn folder_roots(&self) -> Result<Vec<FolderSummary>>;
    /// Immediate subdirectories of `path` that contain at least one track.
    fn child_folders(&self, path: &str) -> Result<Vec<FolderSummary>>;
    /// The ReplayGain tags for one track, or `None` when it has none.
    ///
    /// Read once per track load rather than carried on [`Track`](crate::Track):
    /// the gain is never rendered in a list, and the row type is hot.
    fn replaygain(&self, track_id: i64) -> Result<Option<crate::ReplayGain>>;
    /// Tracks at or below `path`, ordered for display.
    fn tracks_in_folder(&self, path: &str) -> Result<Vec<Track>>;
    /// Tracks sitting *directly* in `path`, in the same order as
    /// [`tracks_in_folder`](Self::tracks_in_folder).
    ///
    /// The folder browser offers these as their own row: without it, a folder
    /// that holds both subdirectories and files hides the files behind the
    /// subdirectory list, because only the subtree is ever reachable.
    fn tracks_directly_in_folder(&self, path: &str) -> Result<Vec<Track>>;
    /// How many tracks sit *directly* in `path`, not in a subdirectory of it.
    ///
    /// Separate from [`tracks_in_folder`](Self::tracks_in_folder), which counts
    /// the whole subtree: a folder browser shows the direct ones as their own row
    /// so a folder that has both subdirectories and loose files does not hide
    /// those files behind the subdirectory list.
    fn direct_track_count(&self, path: &str) -> Result<i64>;
    fn track(&self, id: i64) -> Result<Option<Track>>;
    /// Every track whose id is in `ids`, in whatever order the query returns.
    ///
    /// One query for the whole set rather than [`track`](Self::track) per id:
    /// restoring a persisted playback queue asks for thousands at startup.
    fn tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>>;
    fn all_tracks(&self) -> Result<Vec<Track>>;
    fn track_count(&self) -> Result<i64>;

    /// Replace a track's genre links with `genres`, in order. Names resolve
    /// case-insensitively through `genres.key`, exactly as a scan does, so an
    /// edit and a rescan converge on the same rows. Genres left without any
    /// track are dropped in the same transaction.
    fn set_track_genres(&self, track_id: i64, genres: &[String]) -> Result<()>;
    /// The track's genres in `track_genres.position` order — the read side of
    /// [`set_track_genres`](LibraryRepository::set_track_genres).
    fn track_genres(&self, track_id: i64) -> Result<Vec<String>>;

    fn create_playlist(&self, name: &str) -> Result<i64>;
    fn delete_playlist(&self, playlist_id: i64) -> Result<()>;
    fn rename_playlist(&self, playlist_id: i64, name: &str) -> Result<()>;
    fn playlists(&self) -> Result<Vec<PlaylistSummary>>;
    fn add_track_to_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()>;
    fn add_tracks_to_playlist(&self, playlist_id: i64, track_ids: &[i64]) -> Result<()>;
    fn remove_track_from_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()>;
    fn move_track_in_playlist(&self, playlist_id: i64, from: usize, to: usize) -> Result<()>;
    fn tracks_for_playlist(&self, playlist_id: i64) -> Result<Vec<Track>>;
    fn playlists_containing_track(&self, track_id: i64) -> Result<Vec<i64>>;

    fn lyrics_variants(&self, track_id: i64) -> Result<Vec<StoredLyrics>>;
    fn upsert_lyrics(&self, track_id: i64, text: &str, source: &str, not_found: bool)
    -> Result<()>;

    fn tracks_by_keys(&self, keys: &[(String, i32)]) -> Result<Vec<Track>>;

    /// All `(hash, id)` pairs in `cover_art`. Lets the scan pipeline skip
    /// thumbnail generation for covers that already exist (they survive
    /// `clear()`), which is why the 2nd scan is much faster than the 1st.
    fn cover_art_hashes(&self) -> Result<Vec<(String, i64)>>;

    fn reconcile_local_sources(&self, folders: &[LocalFolder]) -> Result<()>;
    fn has_media_under(&self, root: &str) -> Result<bool>;
    fn has_unplaced_media(&self) -> Result<bool>;
    fn sources(&self) -> Result<Vec<SourceSummary>>;
    fn playback_locators(&self, item_id: i64) -> Result<Vec<(String, i64)>>;
    fn invalidate_scan_fingerprint(&self) -> Result<()>;
    fn refresh_item_snapshots(&self) -> Result<()>;

    /// Open a batched scan-write session on a dedicated connection.
    fn open_scan_session(&self) -> Result<Box<dyn ScanWrite>>;

    /// Fingerprint of the filesystem state captured at the last successful
    /// scan, or `None` if never scanned. See [`set_scan_meta`](LibraryRepository::set_scan_meta).
    fn scan_fingerprint(&self) -> Result<Option<String>>;
    /// Serialized folder set from the last successful scan.
    fn scan_folders(&self) -> Result<Option<String>>;
    /// Persist the fingerprint + folder set after a successful scan.
    fn set_scan_meta(&self, fingerprint: &str, folders: &str) -> Result<()>;

    fn vacuum(&self) -> Result<()>;

    /// Record one listen. Identity is `(started_at, artist, title)`, so a
    /// repeated call for the same listen merges rather than duplicating.
    fn record_play(&self, play: &NewPlay) -> Result<i64>;

    fn play_tallies(&self, since: Option<u64>) -> Result<Vec<PlayTally>>;
    fn recent_plays(&self, limit: usize) -> Result<Vec<RecentPlay>>;

    /// Listening history summarised: totals plus the top artists, albums and
    /// tracks. `since` filters on when the play started.
    fn listening_stats(&self, since: Option<u64>, limit: usize) -> Result<ListeningStats>;
    fn track_listings(&self) -> Result<Vec<TrackListing>>;

    /// Rotated backups on disk, newest first.
    fn backups(&self) -> Vec<crate::backup::BackupEntry>;
    /// Ask for `backup` to replace the live database on the next launch.
    ///
    /// Deliberately not applied immediately: the database is open with WAL
    /// sidecars in this process, and swapping the file underneath it is how a
    /// library gets corrupted. The request is recorded and carried out by
    /// `SqliteLibrary::open`, before anything opens a connection.
    fn request_restore(&self, backup: &std::path::Path) -> Result<()>;
    /// A cheap health check. Counts, never deletes.
    fn integrity_report(&self) -> Result<crate::models::IntegrityReport>;
}
