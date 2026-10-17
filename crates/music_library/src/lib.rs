pub mod adoption;
pub mod album_artists;
pub mod backup;
pub mod error;
pub mod genres;
pub mod migrations;
pub mod models;
pub mod repository;
pub mod sqlite;
pub mod thumbnail;

pub use adoption::normalize_tag;
pub use error::{LibraryError, Result};
pub use genres::normalize_genres;
pub use models::{
    Album, AlbumSearchEntry, AlbumSummary, Artist, ArtistGrouping, ArtistSummary, CoverArt,
    FolderSummary, GenreSort, GenreSummary, IntegrityReport, LocalFolder, NewTrack, PlayTally,
    Playlist, PlaylistSummary, RecentPlay, ListeningStats, StatEntry,
    ScanLyrics, ScanTrack, SourceSummary, StoredLyrics, Track, TrackListing, lyrics_source,
    split_folder_path,
};
pub use repository::{LibraryRepository, ScanWrite};
pub use sqlite::{SqliteLibrary, sha256_hex};

pub const NO_METADATA_ALBUM_ID: i64 = -1;
pub const NO_METADATA_ARTIST_ID: i64 = -2;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn create_test_db() -> (SqliteLibrary, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let temp_dir = std::env::temp_dir().join("pulse-music-library");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join(format!(
            "test-{}-{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&db_path);
        (SqliteLibrary::open_at(&db_path).unwrap(), db_path)
    }

    fn fresh_db_path() -> PathBuf {
        let (lib, path) = create_test_db();
        drop(lib);
        let _ = std::fs::remove_file(&path);
        for suffix in ["-wal", "-shm", ".bak-v8"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(sidecar));
        }
        path
    }

    fn build_v8_db(path: &PathBuf, seed: &str) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for (version, sql) in migrations::MIGRATIONS.iter().filter(|(v, _)| *v <= 8) {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
        }
        conn.execute_batch(seed).unwrap();
    }

    const V8_CATALOG: &str = "
        INSERT INTO artists (id, name, sort_name) VALUES (1, 'Band', 'band');
        INSERT INTO albums (id, title, year) VALUES (1, 'Record', 2001);
        INSERT INTO album_artists (album_id, artist_id, position) VALUES (1, 1, 0);
        INSERT INTO tracks
            (id, path, title, album_id, track_number, duration_ms, start_offset_ms, liked, is_cue)
        VALUES
            (10, '/m/one.flac', 'One', 1, 1, 1000, 0, 1, 0),
            (11, '/m/cue.flac', 'Cue A', 1, 2, 2000, 0, 0, 1),
            (12, '/m/cue.flac', 'Cue B', 1, 3, 2000, 2000, 1, 1);
        INSERT INTO track_artists (track_id, artist_id, position)
            VALUES (10, 1, 0), (11, 1, 0), (12, 1, 0);
    ";

    fn lyric(lib: &SqliteLibrary, track_id: i64, source: &str) -> Option<StoredLyrics> {
        lib.lyrics_variants(track_id)
            .unwrap()
            .into_iter()
            .find(|v| v.source == source)
    }

    #[test]
    fn migration_to_v10_keys_lyrics_by_source_and_keeps_the_guard() {
        let path = fresh_db_path();
        build_v8_db(
            &path,
            &format!(
                "{V8_CATALOG}
                INSERT INTO lyrics (track_id, source, text, not_found, updated_at)
                    VALUES (10, 'lrclib', x'00', 0, 1);"
            ),
        );
        let lib = SqliteLibrary::open_at(&path).unwrap();

        lib.upsert_lyrics(10, "from tag", "embedded", false)
            .unwrap();
        let sources: Vec<String> = lib
            .lyrics_variants(10)
            .unwrap()
            .into_iter()
            .map(|v| v.source)
            .collect();
        assert_eq!(sources, vec!["embedded".to_string(), "lrclib".to_string()]);

        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DELETE FROM tracks WHERE id = 10")
            .unwrap();
        let guarded = conn.execute("DELETE FROM media_items WHERE id = 10", []);
        assert!(guarded.is_err());
    }

    fn user_version(path: &PathBuf) -> i64 {
        count_rows(path, "SELECT user_version FROM pragma_user_version")
    }

    /// The playlist the migration folds the legacy `liked` column into. By the
    /// time the whole chain has run, the v12 cleanup has renamed it out of the
    /// way and removed the `scan_meta` marker, so it is found by name now.
    fn imported_likes_playlist(path: &PathBuf) -> i64 {
        count_rows(
            path,
            "SELECT COALESCE((SELECT id FROM playlists WHERE name = 'Imported likes'), 0)",
        )
    }

    /// Stands in for the old "like" marker. A playlist entry is user data, so it
    /// exercises the same adoption, sweep-guard and re-attach paths the Liked
    /// tab used to.
    fn mark(lib: &SqliteLibrary, track_id: i64) -> i64 {
        let playlist = lib.create_playlist("Kept").unwrap();
        lib.add_track_to_playlist(playlist, track_id).unwrap();
        playlist
    }

    #[test]
    fn migration_to_v9_keeps_every_user_row_under_the_same_id() {
        let path = fresh_db_path();
        build_v8_db(
            &path,
            &format!(
                "{V8_CATALOG}
                INSERT INTO playlists (id, name, created_at) VALUES (5, 'Liked', 0), (6, 'Mix', 0);
                INSERT INTO scan_meta (key, value) VALUES ('liked_playlist_id', '5');
                INSERT INTO playlist_tracks (playlist_id, position, track_id)
                    VALUES (5, 0, 12), (5, 1, 10), (6, 0, 11), (6, 1, 10);
                INSERT INTO lyrics (track_id, source, text, not_found, updated_at)
                    VALUES (10, 'lrclib', x'00', 0, 1), (11, 'lrc', x'00', 0, 1);
                INSERT INTO plays (id, track_id, artist, title, started_at, qualified)
                    VALUES (100, 10, 'Band', 'One', 1000, 1), (101, NULL, 'Gone', 'Old', 900, 1);
                INSERT INTO play_deliveries (play_id, target, state, updated_at)
                    VALUES (100, 'lastfm', 0, 1), (101, 'lastfm', 0, 1);
                INSERT INTO loves (id, track_id, artist, title, loved, at)
                    VALUES (200, 12, 'Band', 'Cue B', 1, 1);
                INSERT INTO love_deliveries (love_id, target, state, updated_at)
                    VALUES (200, 'lastfm', 0, 1);"
            ),
        );

        let lib = SqliteLibrary::open_at(&path).unwrap();

        assert_eq!(
            user_version(&path),
            migrations::MIGRATIONS.last().unwrap().0 as i64
        );
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak-v8");
        assert!(PathBuf::from(backup).exists());

        let mut ids: Vec<i64> = lib.all_tracks().unwrap().iter().map(|t| t.id).collect();
        ids.sort();
        assert_eq!(ids, vec![10, 11, 12]);

        let liked: Vec<i64> = lib
            .tracks_for_playlist(5)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(liked, vec![12, 10]);

        let mix: Vec<i64> = lib
            .tracks_for_playlist(6)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(mix, vec![11, 10]);

        for (sql, expected) in [
            ("SELECT COUNT(*) FROM media_items", 3),
            ("SELECT COUNT(*) FROM media_bindings WHERE source_id = 1", 3),
            (
                "SELECT COUNT(*) FROM lyrics WHERE track_id = 10 AND source = 'lrclib'",
                1,
            ),
            (
                "SELECT COUNT(*) FROM lyrics WHERE track_id = 11 AND source = 'lrc'",
                1,
            ),
            (
                "SELECT COUNT(*) FROM plays WHERE id = 100 AND track_id = 10",
                1,
            ),
            (
                "SELECT COUNT(*) FROM plays WHERE id = 101 AND track_id IS NULL",
                1,
            ),
            (
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'play_deliveries'",
                0,
            ),
            // Pulse 4.0 removes the feature's own tables outright.
            (
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'loves'",
                0,
            ),
            (
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'love_deliveries'",
                0,
            ),
            (
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'view' AND name = 'liked_track_ids'",
                0,
            ),
            (
                "SELECT COUNT(*) FROM scan_meta WHERE key = 'liked_playlist_id'",
                0,
            ),
            ("SELECT COUNT(*) FROM pragma_foreign_key_check", 0),
            (
                "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name = 'liked'",
                0,
            ),
            (
                "SELECT COUNT(*) FROM media_items WHERE id = 12 AND title = 'Cue B' AND artist = 'Band' AND album = 'Record'",
                1,
            ),
        ] {
            assert_eq!(count_rows(&path, sql), expected, "{sql}");
        }
    }

    #[test]
    fn migrated_bindings_count_as_unplaced_until_a_scan_moves_them_to_real_folders() {
        let path = fresh_db_path();
        build_v8_db(&path, V8_CATALOG);
        let lib = SqliteLibrary::open_at(&path).unwrap();
        assert!(lib.has_unplaced_media().unwrap());

        lib.reconcile_local_sources(&folders(&["/m"])).unwrap();
        assert!(lib.has_unplaced_media().unwrap());

        let mut cue_b = scan_track("/m/cue.flac", "Cue B");
        cue_b.start_offset_ms = Some(2000);
        scan(
            &lib,
            vec![
                scan_track("/m/one.flac", "One"),
                scan_track("/m/cue.flac", "Cue A"),
                cue_b,
            ],
        );
        assert!(!lib.has_unplaced_media().unwrap());
        assert_eq!(lib.sources().unwrap()[0].track_count, 3);
    }

    #[test]
    fn migration_folds_the_likes_column_into_a_playlist_and_retires_the_name() {
        // History, not a live feature. Two things have to be true once the whole
        // chain has run: an old database must not lose the tracks somebody had
        // marked, and nothing called "Liked" may survive to look like a feature.
        let path = fresh_db_path();
        build_v8_db(&path, V8_CATALOG);

        let lib = SqliteLibrary::open_at(&path).unwrap();

        let names: Vec<String> = lib.playlists().unwrap().iter().map(|p| p.name.clone()).collect();
        assert!(
            names.iter().all(|n| !n.eq_ignore_ascii_case("liked")),
            "{names:?}"
        );

        let seeded = imported_likes_playlist(&path);
        assert_ne!(seeded, 0, "the likes must survive as an ordinary playlist");
        let liked: Vec<i64> = lib
            .tracks_for_playlist(seeded)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(liked, vec![10, 12]);
    }

    /// The user-visible half of the problem. A database that had the feature but
    /// nothing marked came up with an empty playlist called "Liked", which read
    /// as a feature that had been emptied rather than one that was removed.
    #[test]
    fn an_empty_liked_playlist_is_deleted_not_left_behind() {
        let path = fresh_db_path();
        build_v8_db(
            &path,
            "INSERT INTO playlists (id, name, created_at) VALUES (5, 'Liked', 0);
             INSERT INTO scan_meta (key, value) VALUES ('liked_playlist_id', '5');",
        );

        let lib = SqliteLibrary::open_at(&path).unwrap();

        let names: Vec<String> = lib.playlists().unwrap().iter().map(|p| p.name.clone()).collect();
        assert!(names.is_empty(), "an empty seeded playlist must be gone: {names:?}");
        assert_eq!(
            count_rows(
                &path,
                "SELECT COUNT(*) FROM scan_meta WHERE key = 'liked_playlist_id'"
            ),
            0
        );
    }

    #[test]
    fn a_fresh_database_gets_no_migration_backup() {
        let path = fresh_db_path();
        let lib = SqliteLibrary::open_at(&path).unwrap();
        drop(lib);
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak-v8");
        assert!(!PathBuf::from(backup).exists());
        assert_eq!(
            user_version(&path),
            migrations::MIGRATIONS.last().unwrap().0 as i64
        );
    }

    #[test]
    fn test_migrations_create_schema() {
        let (lib, _path) = create_test_db();
        assert!(!lib.has_tracks().unwrap());
    }

    #[test]
    fn test_upsert_artist() {
        let (lib, _path) = create_test_db();
        let id1 = lib.upsert_artist("The Beatles").unwrap();
        let id2 = lib.upsert_artist("The Beatles").unwrap();
        assert_eq!(id1, id2, "upsert should return same id for same artist");

        let id3 = lib.upsert_artist("Radiohead").unwrap();
        assert_ne!(id1, id3, "different artists should have different ids");
    }

    #[test]
    fn test_upsert_album() {
        let (lib, _path) = create_test_db();
        let album_id = lib.upsert_album("Abbey Road", Some(1969), None).unwrap();
        let album_id2 = lib.upsert_album("Abbey Road", Some(1969), None).unwrap();
        assert_eq!(
            album_id, album_id2,
            "upsert should return same id for same album"
        );
    }

    #[test]
    fn test_album_artists() {
        let (lib, _path) = create_test_db();
        let artist1 = lib.upsert_artist("The Beatles").unwrap();
        let artist2 = lib.upsert_artist("Billy Preston").unwrap();
        let album_id = lib.upsert_album("Let It Be", Some(1970), None).unwrap();
        lib.set_album_artists(album_id, &[(artist1, 0), (artist2, 1)])
            .unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist_name, "The Beatles");
    }

    #[test]
    fn test_upsert_track_and_query() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Radiohead").unwrap();
        let album_id = lib.upsert_album("OK Computer", Some(1997), None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let track = NewTrack {
            path: "/music/01 Airbag.flac".into(),
            title: Some("Airbag".into()),
            album_title: Some("OK Computer".into()),
            artist_names: vec!["Radiohead".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(1997),
            duration_ms: Some(285_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let _track_id = lib
            .upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap();

        let tracks = lib.tracks_for_album(album_id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Airbag");
        assert_eq!(tracks[0].track_number, Some(1));
    }

    #[test]
    fn test_albums_sorted_by_artist_sort_name() {
        let (lib, _path) = create_test_db();
        let beatles = lib.upsert_artist("The Beatles").unwrap();
        let zeppelin = lib.upsert_artist("Led Zeppelin").unwrap();

        let album1 = lib.upsert_album("Abbey Road", Some(1969), None).unwrap();
        let album2 = lib.upsert_album("IV", Some(1971), None).unwrap();

        lib.set_album_artists(album1, &[(beatles, 0)]).unwrap();
        lib.set_album_artists(album2, &[(zeppelin, 0)]).unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].title, "IV");
        assert_eq!(albums[1].title, "Abbey Road");
    }

    #[test]
    fn test_tracks_by_keys_matches_path_and_offset() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let new_track = |path: &str, title: &str, offset: Option<u64>| {
            let t = NewTrack {
                path: path.into(),
                title: Some(title.into()),
                album_title: Some("Album".into()),
                artist_names: vec!["Artist".into()],
                track_number: None,
                disc_number: None,
                year: None,
                duration_ms: None,
                cover_art_id: None,
                start_offset_ms: offset,
                bitrate: None,
            };
            lib.upsert_track(&t, Some(album_id), &[(artist_id, 0)])
                .unwrap()
        };

        new_track("/music/a.flac", "A", None);
        new_track("/album.flac", "Cue 1", Some(0));
        new_track("/album.flac", "Cue 2", Some(5000));

        let found = lib
            .tracks_by_keys(&[
                ("/music/a.flac".into(), 0),
                ("/album.flac".into(), 5000),
                ("/missing.flac".into(), 0),
            ])
            .unwrap();
        let mut rows: Vec<(String, i32)> = found
            .iter()
            .map(|t| (t.title.clone(), t.start_offset_ms))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            vec![
                ("A".to_string(), 0),
                ("Cue 1".to_string(), 0),
                ("Cue 2".to_string(), 5000),
            ]
        );

        assert!(lib.tracks_by_keys(&[]).unwrap().is_empty());
    }

    #[test]
    fn test_all_tracks_ordered_and_counted() {
        let (lib, _path) = create_test_db();
        assert_eq!(lib.track_count().unwrap(), 0);
        assert!(lib.all_tracks().unwrap().is_empty());

        let beatles = lib.upsert_artist("The Beatles").unwrap();
        let radiohead = lib.upsert_artist("Radiohead").unwrap();
        let abbey = lib.upsert_album("Abbey Road", Some(1969), None).unwrap();
        let ok = lib.upsert_album("OK Computer", Some(1997), None).unwrap();
        lib.set_album_artists(abbey, &[(beatles, 0)]).unwrap();
        lib.set_album_artists(ok, &[(radiohead, 0)]).unwrap();

        let add = |path: &str, title: &str, album: &str, artist: &str, album_id, artist_id, no| {
            let t = NewTrack {
                path: path.into(),
                title: Some(title.into()),
                album_title: Some(album.into()),
                artist_names: vec![artist.into()],
                track_number: Some(no),
                disc_number: Some(1),
                year: None,
                duration_ms: None,
                cover_art_id: None,
                start_offset_ms: None,
                bitrate: None,
            };
            lib.upsert_track(&t, Some(album_id), &[(artist_id, 0)])
                .unwrap();
        };

        add(
            "/r/2.flac",
            "Paranoid Android",
            "OK Computer",
            "Radiohead",
            ok,
            radiohead,
            2,
        );
        add(
            "/r/1.flac",
            "Airbag",
            "OK Computer",
            "Radiohead",
            ok,
            radiohead,
            1,
        );
        add(
            "/b/1.flac",
            "Come Together",
            "Abbey Road",
            "The Beatles",
            abbey,
            beatles,
            1,
        );

        assert_eq!(lib.track_count().unwrap(), 3);

        let titles: Vec<String> = lib
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, vec!["Come Together", "Airbag", "Paranoid Android"]);
    }

    #[test]
    fn test_no_metadata_bucket() {
        let (lib, _path) = create_test_db();
        let beatles = lib.upsert_artist("The Beatles").unwrap();
        let abbey = lib.upsert_album("Abbey Road", Some(1969), None).unwrap();
        lib.set_album_artists(abbey, &[(beatles, 0)]).unwrap();

        let tagged = NewTrack {
            path: "/b/1.flac".into(),
            title: Some("Come Together".into()),
            album_title: Some("Abbey Road".into()),
            artist_names: vec!["The Beatles".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&tagged, Some(abbey), &[(beatles, 0)])
            .unwrap();

        assert!(
            !lib.albums()
                .unwrap()
                .iter()
                .any(|a| a.id == NO_METADATA_ALBUM_ID)
        );
        assert!(
            !lib.artists(ArtistGrouping::TrackArtist)
                .unwrap()
                .iter()
                .any(|a| a.id == NO_METADATA_ARTIST_ID)
        );

        let bare = NewTrack {
            path: "/loose/track.flac".into(),
            title: Some("track".into()),
            album_title: None,
            artist_names: Vec::new(),
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&bare, None, &[]).unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.last().map(|a| a.id), Some(NO_METADATA_ALBUM_ID));
        let album_tracks = lib.tracks_for_album(NO_METADATA_ALBUM_ID).unwrap();
        assert_eq!(album_tracks.len(), 1);
        assert_eq!(album_tracks[0].path, "/loose/track.flac");

        let artists = lib.artists(ArtistGrouping::TrackArtist).unwrap();
        let no_meta_artist = artists.iter().find(|a| a.id == NO_METADATA_ARTIST_ID);
        assert_eq!(no_meta_artist.map(|a| a.track_count), Some(1));
        let artist_tracks = lib
            .tracks_by_artist(NO_METADATA_ARTIST_ID, ArtistGrouping::TrackArtist)
            .unwrap();
        assert_eq!(artist_tracks.len(), 1);
        assert_eq!(artist_tracks[0].path, "/loose/track.flac");
    }

    #[test]
    fn test_tracks_by_keys_chunks_beyond_parameter_limit() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let mut keys: Vec<(String, i32)> = Vec::new();
        for i in 0..1100 {
            let path = format!("/music/{i}.flac");
            let track = NewTrack {
                path: path.clone(),
                title: Some(format!("T{i}")),
                album_title: Some("Album".into()),
                artist_names: vec!["Artist".into()],
                track_number: None,
                disc_number: None,
                year: None,
                duration_ms: None,
                cover_art_id: None,
                start_offset_ms: None,
                bitrate: None,
            };
            lib.upsert_track(&track, Some(album_id), &[(artist_id, 0)])
                .unwrap();
            keys.push((path, 0));
        }

        let found = lib.tracks_by_keys(&keys).unwrap();
        assert_eq!(found.len(), 1100);
    }

    #[test]
    fn test_clear() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let track = NewTrack {
            path: "/music/song.flac".into(),
            title: Some("Song".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap();

        assert!(lib.has_tracks().unwrap());
        lib.clear().unwrap();
        assert!(!lib.has_tracks().unwrap());
    }

    #[test]
    fn test_track_title_fallback_from_path() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let track = NewTrack {
            path: "/music/Unknown Title.flac".into(),
            title: None,
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let _track_id = lib
            .upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap();
        let tracks = lib.tracks_for_album(album_id).unwrap();
        assert_eq!(tracks[0].title, "Unknown Title");
    }

    #[test]
    fn test_multidisc_tracks_ordered_by_disc() {
        let (lib, _path) = create_test_db();
        let album_artist_id = lib.upsert_artist("Album Artist").unwrap();
        let track1_artist_id = lib.upsert_artist("Artist One").unwrap();
        let track2_artist_id = lib.upsert_artist("Artist Two").unwrap();
        let album_id = lib
            .upsert_album("Multi-Disc Album", Some(2020), None)
            .unwrap();

        let track1 = NewTrack {
            path: "/music/disc1/track01.flac".into(),
            title: Some("Track One".into()),
            album_title: Some("Multi-Disc Album".into()),
            artist_names: vec!["Artist One".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2020),
            duration_ms: Some(180_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let track2 = NewTrack {
            path: "/music/disc2/track01.flac".into(),
            title: Some("Track Two".into()),
            album_title: Some("Multi-Disc Album".into()),
            artist_names: vec!["Artist Two".into()],
            track_number: Some(1),
            disc_number: Some(2),
            year: Some(2020),
            duration_ms: Some(200_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };

        lib.upsert_track(&track1, Some(album_id), &[(track1_artist_id, 0)])
            .unwrap();
        lib.upsert_track(&track2, Some(album_id), &[(track2_artist_id, 0)])
            .unwrap();
        lib.set_album_artists(album_id, &[(album_artist_id, 0)])
            .unwrap();

        let tracks = lib.tracks_for_album(album_id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].disc_number, 1);
        assert_eq!(tracks[0].title, "Track One");
        assert_eq!(tracks[1].disc_number, 2);
        assert_eq!(tracks[1].title, "Track Two");

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist_name, "Album Artist");
    }

    #[test]
    fn test_track_artists() {
        let (lib, _path) = create_test_db();
        let artist1 = lib.upsert_artist("Artist One").unwrap();
        let artist2 = lib.upsert_artist("Artist Two").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();

        let track = NewTrack {
            path: "/music/track.flac".into(),
            title: Some("Track".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist One".into(), "Artist Two".into()],
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let track_id = lib
            .upsert_track(&track, Some(album_id), &[(artist1, 0), (artist2, 1)])
            .unwrap();
        let artists = lib.track_artists(track_id).unwrap();
        assert_eq!(artists, vec!["Artist One", "Artist Two"]);
    }

    #[test]
    fn test_album_title_found() {
        let (lib, _path) = create_test_db();
        let album_id = lib.upsert_album("Test Album", Some(2000), None).unwrap();
        assert_eq!(
            lib.album_title(album_id).unwrap(),
            Some("Test Album".into())
        );
    }

    #[test]
    fn test_album_title_not_found() {
        let (lib, _path) = create_test_db();
        assert!(lib.album_title(999).unwrap().is_none());
    }

    #[test]
    fn test_vacuum_preserves_data() {
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");

        lib.vacuum().unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 1);
        let tracks = lib.tracks_for_album(albums[0].id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, track_id);
        assert_eq!(tracks[0].title, "Song");
    }

    /// The whole point of a backup: what it writes can be opened on its own. A
    /// `VACUUM INTO` copy carries no `-wal`, so opening it standalone and
    /// reading the rows back is the real assertion — a byte copy of a live WAL
    /// database fails exactly here.
    #[test]
    fn daily_backup_is_restorable_and_happens_once_a_day() {
        let (lib, path) = create_test_db();
        seed_track(&lib, "Song", "Album", "Artist");
        let (dir, stem) = backup::backup_location(&path).unwrap();

        let first = lib
            .daily_backup(backup::DEFAULT_KEEP_DAILY_BACKUPS)
            .unwrap()
            .expect("the first launch of the day writes a copy");
        assert!(first.exists());

        let restored = SqliteLibrary::open_at(&first).unwrap();
        assert_eq!(restored.track_count().unwrap(), 1);

        // A second launch the same day is a no-op and adds no file. Counted by
        // stem, so a parallel test's backups cannot make this flaky.
        assert!(
            lib.daily_backup(backup::DEFAULT_KEEP_DAILY_BACKUPS)
                .unwrap()
                .is_none()
        );
        let ours = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                backup::is_backup_name(&stem, &name.to_string_lossy())
            })
            .count();
        assert_eq!(ours, 1, "a second launch the same day must not add a file");
    }

    #[test]
    fn daily_backup_prunes_to_the_window_and_keeps_one_per_month() {
        let (lib, path) = create_test_db();
        seed_track(&lib, "Song", "Album", "Artist");
        let (dir, stem) = backup::backup_location(&path).unwrap();
        std::fs::create_dir_all(&dir).unwrap();

        let day = |date: &str| format!("{stem}-{date}.db");
        // Five consecutive days in one past month, then one each in two older
        // months, plus a file that is not ours.
        for name in [
            day("20260305"),
            day("20260304"),
            day("20260303"),
            day("20260302"),
            day("20260301"),
            day("20260201"),
            day("20260101"),
        ] {
            std::fs::write(dir.join(&name), b"old").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"keep me").unwrap();
        // A copy that died mid-write: the partial and its rollback journal.
        std::fs::write(
            dir.join(format!("{}.partial", day("20260201"))),
            b"junk",
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("{}.partial-journal", day("20260201"))),
            b"junk",
        )
        .unwrap();

        lib.daily_backup(1).unwrap();

        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        assert!(names.contains(&"notes.txt".to_string()), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains(".partial")),
            "a dead partial copy and its journal must be swept: {names:?}"
        );

        // Today, the newest day of the month that falls inside the window, and
        // one survivor for each older month.
        assert!(names.contains(&day("20260305")), "{names:?}");
        assert!(names.contains(&day("20260201")), "{names:?}");
        assert!(names.contains(&day("20260101")), "{names:?}");
        assert!(!names.contains(&day("20260304")), "{names:?}");
        assert!(!names.contains(&day("20260301")), "{names:?}");
        let ours = names
            .iter()
            .filter(|name| backup::is_backup_name(&stem, name))
            .count();
        assert_eq!(ours, 4, "{names:?}");
    }

    #[test]
    fn test_delete_orphaned_albums_and_artists() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Orphan Artist").unwrap();
        let album_id = lib.upsert_album("Orphan Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        lib.delete_orphaned_albums_and_artists().unwrap();

        assert!(lib.album_title(album_id).unwrap().is_none());
        assert!(lib.album_artists(album_id).unwrap().is_empty());
    }

    #[test]
    fn test_save_and_retrieve_cover_art() {
        let (lib, _path) = create_test_db();
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        let data = buf.into_inner();

        let id = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();
        assert!(id > 0);

        let id2 = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();
        assert_eq!(id, id2);

        let cover = lib.get_cover_art(id).unwrap().unwrap();
        assert!(!cover.small.is_empty());
        assert!(!cover.large.is_empty());
    }

    #[test]
    fn test_clear_then_reinsert() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();
        let track = NewTrack {
            path: "/music/song.flac".into(),
            title: Some("Song".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap();
        assert!(lib.has_tracks().unwrap());

        lib.clear().unwrap();
        assert!(!lib.has_tracks().unwrap());

        let artist_id2 = lib.upsert_artist("Artist").unwrap();
        let album_id2 = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id2, &[(artist_id2, 0)])
            .unwrap();
        lib.upsert_track(&track, Some(album_id2), &[(artist_id2, 0)])
            .unwrap();
        assert!(lib.has_tracks().unwrap());
        let tracks = lib.tracks_for_album(album_id2).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Song");
    }

    #[test]
    fn test_empty_album_artist_in_summary() {
        let (lib, _path) = create_test_db();
        let album_id = lib.upsert_album("Compilation", Some(2020), None).unwrap();
        let artist_id = lib.upsert_artist("Various").unwrap();
        let track = NewTrack {
            path: "/music/track01.flac".into(),
            title: Some("Track".into()),
            album_title: Some("Compilation".into()),
            artist_names: vec!["Various".into()],
            track_number: Some(1),
            disc_number: None,
            year: Some(2020),
            duration_ms: Some(180_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap();

        let albums = lib.albums().unwrap();
        let album = albums.iter().find(|a| a.id == album_id).unwrap();
        assert_eq!(album.artist_name, "");
    }

    #[test]
    fn test_track_without_album() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let track = NewTrack {
            path: "/music/song.flac".into(),
            title: Some("Song".into()),
            album_title: None,
            artist_names: vec!["Artist".into()],
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let track_id = lib.upsert_track(&track, None, &[(artist_id, 0)]).unwrap();

        assert!(lib.has_tracks().unwrap());
        let artists = lib.track_artists(track_id).unwrap();
        assert_eq!(artists, vec!["Artist"]);
    }

    #[test]
    fn test_same_path_different_offset() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib.upsert_album("Album", None, None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let track1 = NewTrack {
            path: "/music/track.flac".into(),
            title: Some("Track One".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: Some(1),
            disc_number: None,
            year: None,
            duration_ms: Some(300_000),
            cover_art_id: None,
            start_offset_ms: Some(0),
            bitrate: None,
        };
        let track2 = NewTrack {
            path: "/music/track.flac".into(),
            title: Some("Track Two".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: Some(2),
            disc_number: None,
            year: None,
            duration_ms: Some(300_000),
            cover_art_id: None,
            start_offset_ms: Some(300_000),
            bitrate: None,
        };

        let id1 = lib
            .upsert_track(&track1, Some(album_id), &[(artist_id, 0)])
            .unwrap();
        let id2 = lib
            .upsert_track(&track2, Some(album_id), &[(artist_id, 0)])
            .unwrap();

        assert_ne!(
            id1, id2,
            "same path with different offsets should create distinct tracks"
        );

        let tracks = lib.tracks_for_album(album_id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "Track One");
        assert_eq!(tracks[1].title, "Track Two");
    }

    #[test]
    fn test_has_tracks_empty_on_fresh_db() {
        let (lib, _path) = create_test_db();
        assert!(!lib.has_tracks().unwrap());
    }

    fn make_test_jpeg(bytes: &[u8]) -> Vec<u8> {
        let color = if bytes.is_empty() {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([
                bytes[0],
                bytes.get(1).copied().unwrap_or(0),
                bytes.get(2).copied().unwrap_or(0),
            ])
        };
        let img = image::RgbImage::from_pixel(4, 4, color);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn test_cover_art_different_images_different_ids() {
        let (lib, _path) = create_test_db();
        let data1 = make_test_jpeg(&[255, 0, 0]);
        let data2 = make_test_jpeg(&[0, 255, 0]);

        let id1 = lib.save_cover_art(&data1, "/music/a.flac", true).unwrap();
        let id2 = lib.save_cover_art(&data2, "/music/a.flac", true).unwrap();
        assert_ne!(id1, id2, "different images must have different IDs");
    }

    #[test]
    fn test_cover_art_get_nonexistent() {
        let (lib, _path) = create_test_db();
        assert!(lib.get_cover_art(999).unwrap().is_none());
        assert!(lib.get_cover_art_small(999).unwrap().is_none());
        assert!(lib.get_cover_art_large(999).unwrap().is_none());
    }

    #[test]
    fn test_cover_art_thumbnail_sizes() {
        let (lib, _path) = create_test_db();
        let data = make_test_jpeg(&[255, 0, 0]);
        let id = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();
        let cover = lib.get_cover_art(id).unwrap().unwrap();

        let small_img = image::load_from_memory(&cover.small).unwrap();
        let large_img = image::load_from_memory(&cover.large).unwrap();
        assert!(small_img.width() <= 128);
        assert!(small_img.height() <= 128);
        assert!(large_img.width() <= 320);
        assert!(large_img.height() <= 320);
    }

    #[test]
    fn test_cover_art_id_propagates_to_album_and_track() {
        let (lib, _path) = create_test_db();
        let data = make_test_jpeg(&[255, 0, 0]);
        let cover_id = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();

        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album_id = lib
            .upsert_album("Album", Some(2020), Some(cover_id))
            .unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        let track = NewTrack {
            path: "/music/song.flac".into(),
            title: Some("Song".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2020),
            duration_ms: Some(200_000),
            cover_art_id: Some(cover_id),
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums[0].cover_art_id, Some(cover_id));

        let tracks = lib.tracks_for_album(album_id).unwrap();
        assert_eq!(tracks[0].cover_art_id, Some(cover_id));

        let retrieved = lib.get_cover_art(cover_id).unwrap().unwrap();
        assert_eq!(retrieved.id, cover_id);
        assert!(!retrieved.small.is_empty());
        assert!(!retrieved.large.is_empty());
    }

    /// The album takes the cover of its *lowest-numbered* track, not of whichever one
    /// was written first — the tracks here go in back to front to prove it, since the
    /// parallel scan gives no order guarantee at all.
    #[test]
    fn test_resolve_album_covers_picks_the_lowest_numbered_track() {
        let (lib, _path) = create_test_db();
        let artist = lib.upsert_artist("Artist").unwrap();
        let album = lib.upsert_album("Album", Some(2020), None).unwrap();

        let mut covers = Vec::new();
        for (n, shade) in [(3u32, 60u8), (2, 30), (1, 0)] {
            let cover = lib
                .save_cover_art(&make_test_jpeg(&[shade, 0, 0]), "/music/a.flac", true)
                .unwrap();
            covers.push((n, cover));
            let track = NewTrack {
                path: format!("/music/{n}.flac"),
                title: Some(format!("Track {n}")),
                album_title: Some("Album".into()),
                artist_names: vec!["Artist".into()],
                track_number: Some(n),
                disc_number: Some(1),
                year: Some(2020),
                duration_ms: Some(1000),
                cover_art_id: Some(cover),
                start_offset_ms: None,
                bitrate: None,
            };
            lib.upsert_track(&track, Some(album), &[(artist, 0)])
                .unwrap();
        }

        lib.resolve_album_covers().unwrap();
        let first_track_cover = covers.iter().find(|(n, _)| *n == 1).unwrap().1;
        assert_eq!(
            lib.albums().unwrap()[0].cover_art_id,
            Some(first_track_cover),
            "insert order was 3, 2, 1 — the result must not depend on it"
        );
    }

    #[test]
    fn test_resolve_album_covers_clears_an_album_whose_tracks_lost_their_art() {
        let (lib, _path) = create_test_db();
        let cover = lib
            .save_cover_art(&make_test_jpeg(&[255, 0, 0]), "/music/a.flac", true)
            .unwrap();
        let artist = lib.upsert_artist("Artist").unwrap();
        let album = lib.upsert_album("Album", None, Some(cover)).unwrap();
        let track = NewTrack {
            path: "/music/one.flac".into(),
            title: Some("One".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: None,
            duration_ms: Some(1000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album), &[(artist, 0)])
            .unwrap();

        lib.resolve_album_covers().unwrap();
        assert_eq!(
            lib.albums().unwrap()[0].cover_art_id,
            None,
            "no track carries art, so the album must not keep a stale cover"
        );
    }

    #[test]
    fn test_cover_art_deduplication_across_albums() {
        let (lib, _path) = create_test_db();
        let data = make_test_jpeg(&[255, 0, 0]);

        let cover_id = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();
        let cover_id2 = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();
        assert_eq!(cover_id, cover_id2, "same bytes must return same ID");

        let artist_id = lib.upsert_artist("Artist").unwrap();
        let album1 = lib.upsert_album("Album 1", None, Some(cover_id)).unwrap();
        let album2 = lib.upsert_album("Album 2", None, Some(cover_id)).unwrap();

        let track1 = NewTrack {
            path: "/music/track1.flac".into(),
            title: Some("Track 1".into()),
            album_title: Some("Album 1".into()),
            artist_names: vec!["Artist".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: None,
            duration_ms: None,
            cover_art_id: Some(cover_id),
            start_offset_ms: None,
            bitrate: None,
        };
        let track2 = NewTrack {
            path: "/music/track2.flac".into(),
            title: Some("Track 2".into()),
            album_title: Some("Album 2".into()),
            artist_names: vec!["Artist".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: None,
            duration_ms: None,
            cover_art_id: Some(cover_id),
            start_offset_ms: None,
            bitrate: None,
        };

        lib.upsert_track(&track1, Some(album1), &[(artist_id, 0)])
            .unwrap();
        lib.upsert_track(&track2, Some(album2), &[(artist_id, 0)])
            .unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 2);
        for album in &albums {
            assert_eq!(album.cover_art_id, Some(cover_id));
        }
    }

    #[test]
    fn test_cover_art_clear_removes_cover_art() {
        let (lib, _path) = create_test_db();
        let data = make_test_jpeg(&[255, 0, 0]);
        let cover_id = lib.save_cover_art(&data, "/music/a.flac", true).unwrap();

        assert!(lib.get_cover_art(cover_id).unwrap().is_some());

        // clear() keeps cover_art rows intact (hash→id mapping stays valid for
        // PlaybackQueue across rescans). Orphans are removed in the post-rescan
        // cleanup step.
        lib.clear().unwrap();
        lib.delete_orphaned_albums_and_artists().unwrap();

        assert!(lib.get_cover_art(cover_id).unwrap().is_none());
    }

    #[test]
    fn test_cover_art_full_scanner_flow() {
        let (lib, _path) = create_test_db();
        let cover_data = make_test_jpeg(&[100, 150, 200]);

        // Step 1: Scanner extracted raw cover bytes
        let cover_art_id = lib
            .save_cover_art(&cover_data, "/music/a.flac", true)
            .unwrap();

        // Step 2: Upsert artist
        let artist_id = lib.upsert_artist("Test Artist").unwrap();

        // Step 3: Upsert album with cover_art_id
        let album_id = lib
            .upsert_album("Test Album", Some(2024), Some(cover_art_id))
            .unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();

        // Step 4: Build NewTrack with cover_art_id (no raw bytes)
        let new_track = NewTrack {
            path: "/music/test.flac".into(),
            title: Some("Test Track".into()),
            album_title: Some("Test Album".into()),
            artist_names: vec!["Test Artist".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2024),
            duration_ms: Some(180_000),
            cover_art_id: Some(cover_art_id),
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&new_track, Some(album_id), &[(artist_id, 0)])
            .unwrap();

        // Step 5: Verify album summary has cover_art_id
        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Test Album");
        assert_eq!(albums[0].cover_art_id, Some(cover_art_id));
        assert_eq!(albums[0].artist_name, "Test Artist");

        // Step 6: Verify track has cover_art_id
        let tracks = lib.tracks_for_album(album_id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].cover_art_id, Some(cover_art_id));

        // Step 7: Verify thumbnails exist and are non-empty
        let cover = lib.get_cover_art(cover_art_id).unwrap().unwrap();
        assert!(!cover.small.is_empty());
        assert!(!cover.large.is_empty());
        assert_eq!(cover.id, cover_art_id);

        // Step 8: Verify small and large can be retrieved independently
        let small = lib.get_cover_art_small(cover_art_id).unwrap().unwrap();
        let large = lib.get_cover_art_large(cover_art_id).unwrap().unwrap();
        assert_eq!(small, cover.small);
        assert_eq!(large, cover.large);
    }

    fn seed_track(lib: &SqliteLibrary, title: &str, album: &str, artist: &str) -> i64 {
        let artist_id = lib.upsert_artist(artist).unwrap();
        let album_id = lib.upsert_album(album, Some(2020), None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();
        let track = NewTrack {
            path: format!("/music/{}.flac", title),
            title: Some(title.into()),
            album_title: Some(album.into()),
            artist_names: vec![artist.into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2020),
            duration_ms: Some(180_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album_id), &[(artist_id, 0)])
            .unwrap()
    }

    /// Record a qualified play for `track_id` at `started_at`.
    ///
    /// `title` is passed explicitly because `record_play` deduplicates on
    /// `(started_at, artist, title)`: two tracks sharing a title would otherwise
    /// merge into one play.
    fn play_track(lib: &SqliteLibrary, track_id: i64, title: &str, started_at: u64) {
        lib.record_play(&models::NewPlay {
            track_id: Some(track_id),
            artist: "Artist".into(),
            title: title.to_owned(),
            album: Some("Album".into()),
            album_artist: Some("Artist".into()),
            track_number: Some(1),
            duration_secs: Some(180),
            played_secs: Some(180),
            started_at,
            qualified: true,
        })
        .unwrap();
    }

    #[test]
    fn listening_stats_summarise_the_history_and_respect_since() {
        let (lib, _path) = create_test_db();
        assert!(lib.listening_stats(None, 10).unwrap().is_empty());

        let first = seed_track(&lib, "A", "Album", "Artist");
        let second = seed_track(&lib, "B", "Album", "Artist");
        play_track(&lib, first, "A", 1_000);
        play_track(&lib, first, "A", 2_000);
        play_track(&lib, second, "B", 3_000);

        let all = lib.listening_stats(None, 10).unwrap();
        assert_eq!(all.plays, 3);
        assert_eq!(all.tracks, 2, "two distinct tracks played");
        assert_eq!(all.albums, 1);
        assert_eq!(all.artists, 1);
        assert_eq!(all.days, 1, "all three plays are on the same day");
        assert_eq!(all.top_artists[0].name, "Artist");
        assert_eq!(all.top_artists[0].plays, 3);
        assert_eq!(all.top_tracks[0].name, "A");
        assert_eq!(all.top_tracks[0].plays, 2);

        let recent = lib.listening_stats(Some(2_500), 10).unwrap();
        assert_eq!(recent.plays, 1);
        assert_eq!(recent.top_tracks[0].name, "B");
        assert_eq!(recent.top_albums[0].subtitle.as_deref(), Some("Artist"));
    }

    #[test]
    fn playlist_reorder_persists_and_containment_is_reported() {
        let (lib, _path) = create_test_db();
        let a = seed_track(&lib, "A", "Album", "Artist");
        let b = seed_track(&lib, "B", "Album", "Artist");
        let c = seed_track(&lib, "C", "Album", "Artist");
        let pl = lib.create_playlist("Mine").unwrap();
        lib.add_tracks_to_playlist(pl, &[a, b, c]).unwrap();

        let ids = |lib: &SqliteLibrary| -> Vec<i64> {
            lib.tracks_for_playlist(pl)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect()
        };
        assert_eq!(ids(&lib), vec![a, b, c]);

        lib.move_track_in_playlist(pl, 0, 2).unwrap();
        assert_eq!(ids(&lib), vec![b, c, a]);

        assert_eq!(lib.playlists().unwrap().len(), 1);
        assert_eq!(lib.playlists_containing_track(b).unwrap(), vec![pl]);

        lib.remove_track_from_playlist(pl, c).unwrap();
        assert!(!ids(&lib).contains(&c));
    }

    #[test]
    fn playlist_add_many_appends_in_order_and_skips_duplicates() {
        let (lib, _path) = create_test_db();
        let a = seed_track(&lib, "A", "Album", "Artist");
        let b = seed_track(&lib, "B", "Album", "Artist");
        let c = seed_track(&lib, "C", "Album", "Artist");
        let pl = lib.create_playlist("Mine").unwrap();
        lib.add_tracks_to_playlist(pl, &[a]).unwrap();

        lib.add_tracks_to_playlist(pl, &[b, c, a]).unwrap();

        let ids: Vec<i64> = lib
            .tracks_for_playlist(pl)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![a, b, c]);

        lib.add_tracks_to_playlist(pl, &[]).unwrap();
        let ids: Vec<i64> = lib
            .tracks_for_playlist(pl)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn test_artists_enumerates_with_track_counts() {
        let (lib, _path) = create_test_db();
        seed_track(&lib, "Song A", "Album X", "Artist Alpha");
        seed_track(&lib, "Song B", "Album X", "Artist Alpha");
        seed_track(&lib, "Song C", "Album Y", "Artist Beta");

        let artists = lib.artists(ArtistGrouping::TrackArtist).unwrap();
        let alpha = artists.iter().find(|a| a.name == "Artist Alpha").unwrap();
        let beta = artists.iter().find(|a| a.name == "Artist Beta").unwrap();
        assert_eq!(alpha.track_count, 2);
        assert_eq!(beta.track_count, 1);
        // Artists with zero tracks should not appear.
        lib.upsert_artist("Lonely Artist").unwrap();
        let artists2 = lib.artists(ArtistGrouping::TrackArtist).unwrap();
        assert!(artists2.iter().all(|a| a.name != "Lonely Artist"));
    }

    #[test]
    fn test_tracks_by_artist_returns_all_albums_for_artist() {
        let (lib, _path) = create_test_db();
        let radiohead = lib.upsert_artist("Radiohead").unwrap();
        let other = lib.upsert_artist("Other").unwrap();
        let ok_computer = lib.upsert_album("OK Computer", Some(1997), None).unwrap();
        let kid_a = lib.upsert_album("Kid A", Some(2000), None).unwrap();
        let other_album = lib.upsert_album("Other Album", Some(2001), None).unwrap();
        lib.set_album_artists(ok_computer, &[(radiohead, 0)])
            .unwrap();
        lib.set_album_artists(kid_a, &[(radiohead, 0)]).unwrap();
        lib.set_album_artists(other_album, &[(other, 0)]).unwrap();

        let mk_track = |path: &str, title: &str, _album_id: i64| NewTrack {
            path: path.into(),
            title: Some(title.into()),
            album_title: None,
            artist_names: vec![],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2000),
            duration_ms: Some(180_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(
            &mk_track("/p/airbag.flac", "Airbag", ok_computer),
            Some(ok_computer),
            &[(radiohead, 0)],
        )
        .unwrap();
        lib.upsert_track(
            &mk_track("/p/everything.flac", "Everything In Its Right Place", kid_a),
            Some(kid_a),
            &[(radiohead, 0)],
        )
        .unwrap();
        lib.upsert_track(
            &mk_track("/p/other.flac", "Other Song", other_album),
            Some(other_album),
            &[(other, 0)],
        )
        .unwrap();

        let tracks = lib
            .tracks_by_artist(radiohead, ArtistGrouping::TrackArtist)
            .unwrap();
        assert_eq!(tracks.len(), 2);
        // Ordered by album year ASC: OK Computer (1997) first, Kid A (2000) second.
        assert_eq!(tracks[0].title, "Airbag");
        assert_eq!(tracks[1].title, "Everything In Its Right Place");
        // Other artist's track is not returned.
        assert!(tracks.iter().all(|t| t.title != "Other Song"));
    }

    #[test]
    fn test_track_artists_map_returns_names_in_position_order() {
        let (lib, _path) = create_test_db();
        let a1 = lib.upsert_artist("Lead").unwrap();
        let a2 = lib.upsert_artist("Feat").unwrap();
        let album = lib.upsert_album("Album", None, None).unwrap();
        let track = NewTrack {
            path: "/music/t.flac".into(),
            title: Some("Track".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Lead".into(), "Feat".into()],
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let id = lib
            .upsert_track(&track, Some(album), &[(a1, 0), (a2, 1)])
            .unwrap();
        let map = lib.track_artists_map(&[id]).unwrap();
        assert_eq!(
            map.get(&id).unwrap(),
            &vec!["Lead".to_string(), "Feat".to_string()]
        );
    }

    #[test]
    fn test_track_artists_map_empty_input() {
        let (lib, _path) = create_test_db();
        let map = lib.track_artists_map(&[]).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn test_delete_playlist_cascades_to_playlist_tracks() {
        // Verifies the FK pragma + ON DELETE CASCADE: dropping a playlist
        // must take its membership rows with it. Without `PRAGMA foreign_keys
        // = ON` this silently leaves orphaned playlist_tracks rows.
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");
        let playlist_id = lib.create_playlist("My Playlist").unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();
        assert_eq!(lib.tracks_for_playlist(playlist_id).unwrap().len(), 1);

        lib.delete_playlist(playlist_id).unwrap();
        // The membership row is gone — playlists_containing_track must not
        // return the deleted playlist id.
        assert!(lib.playlists_containing_track(track_id).unwrap().is_empty());
    }

    #[test]
    fn test_rename_playlist_keeps_its_tracks() {
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");
        let playlist_id = lib.create_playlist("Old").unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();

        lib.rename_playlist(playlist_id, "New").unwrap();

        let playlists = lib.playlists().unwrap();
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, playlist_id);
        assert_eq!(playlists[0].name, "New");
        assert_eq!(playlists[0].track_count, 1);
    }

    #[test]
    fn test_add_track_to_playlist_is_idempotent() {
        // Double-click / stale `containing` UI checks should be harmless.
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");
        let playlist_id = lib.create_playlist("My Playlist").unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();
        assert_eq!(lib.tracks_for_playlist(playlist_id).unwrap().len(), 1);
    }

    #[test]
    fn test_add_tracks_to_playlist_appends_in_order_and_skips_present() {
        let (lib, _path) = create_test_db();
        let a = seed_track(&lib, "A", "Album", "Artist");
        let b = seed_track(&lib, "B", "Album", "Artist");
        let c = seed_track(&lib, "C", "Album", "Artist");
        let playlist_id = lib.create_playlist("Batch").unwrap();
        lib.add_track_to_playlist(playlist_id, b).unwrap();
        lib.add_tracks_to_playlist(playlist_id, &[a, b, c, a])
            .unwrap();
        lib.add_tracks_to_playlist(playlist_id, &[]).unwrap();

        let ids: Vec<i64> = lib
            .tracks_for_playlist(playlist_id)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![b, a, c]);

        lib.move_track_in_playlist(playlist_id, 2, 0).unwrap();
        let ids: Vec<i64> = lib
            .tracks_for_playlist(playlist_id)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![c, b, a]);
    }

    #[test]
    fn test_add_tracks_to_playlist_skips_missing_tracks() {
        let (lib, _path) = create_test_db();
        let a = seed_track(&lib, "A", "Album", "Artist");
        let b = seed_track(&lib, "B", "Album", "Artist");
        let missing = a.max(b) + 1000;
        let playlist_id = lib.create_playlist("Stale").unwrap();
        lib.add_tracks_to_playlist(playlist_id, &[a, missing, b])
            .unwrap();

        let ids: Vec<i64> = lib
            .tracks_for_playlist(playlist_id)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![a, b]);
    }

    #[test]
    fn test_move_track_in_playlist_reorders_positions() {
        let (lib, _path) = create_test_db();
        let a = seed_track(&lib, "A", "Album", "Artist");
        let b = seed_track(&lib, "B", "Album", "Artist");
        let c = seed_track(&lib, "C", "Album", "Artist");
        let playlist_id = lib.create_playlist("Order").unwrap();
        lib.add_track_to_playlist(playlist_id, a).unwrap();
        lib.add_track_to_playlist(playlist_id, b).unwrap();
        lib.add_track_to_playlist(playlist_id, c).unwrap();

        let ids = |lib: &SqliteLibrary| -> Vec<i64> {
            lib.tracks_for_playlist(playlist_id)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect()
        };

        lib.move_track_in_playlist(playlist_id, 0, 2).unwrap();
        assert_eq!(ids(&lib), vec![b, c, a]);

        lib.move_track_in_playlist(playlist_id, 2, 0).unwrap();
        assert_eq!(ids(&lib), vec![a, b, c]);

        lib.move_track_in_playlist(playlist_id, 1, 1).unwrap();
        assert_eq!(ids(&lib), vec![a, b, c]);

        lib.move_track_in_playlist(playlist_id, 0, 9).unwrap();
        assert_eq!(ids(&lib), vec![a, b, c]);
    }

    fn content_size(title: &str) -> u64 {
        title.bytes().fold(1_000_003u64, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as u64)
        }) % 1_000_000_000
    }

    fn scan_track(path: &str, title: &str) -> ScanTrack {
        ScanTrack {
            path: path.into(),
            title: Some(title.into()),
            file_size: Some(content_size(title)),
            album_title: Some("Album".into()),
            artist_names: vec!["Artist".into()],
            album_artist_names: vec!["Artist".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2020),
            duration_ms: Some(180_000),
            ..Default::default()
        }
    }

    fn scan(lib: &SqliteLibrary, tracks: Vec<ScanTrack>) {
        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        for track in tracks {
            session.add_track(track).unwrap();
        }
        session.finish().unwrap();
    }

    #[test]
    fn a_track_knows_where_it_lives() {
        let track = |path: &str, is_cue: bool| Track {
            id: 1,
            path: path.into(),
            title: String::new(),
            album_id: None,
            track_number: None,
            disc_number: 1,
            duration_ms: None,
            year: None,
            cover_art_id: None,
            start_offset_ms: 0,
            bitrate: None,
            is_cue,
            available: true,
        };
        let file = track("/m/a.flac", false);
        assert_eq!(file.own_file(), Some(std::path::Path::new("/m/a.flac")));
        let piece = track("/m/disc.flac", true);
        assert!(piece.local_file().is_some());
        assert_eq!(piece.own_file(), None);
        // A locator a pre-3.0 build wrote for a media-server track points at no
        // file, so it must not reach the tag editor or the decoder.
        let legacy = track("pawse-source://3/k.flac", false);
        assert_eq!(legacy.local_file(), None);
        assert_eq!(legacy.own_file(), None);
    }

    fn on_album(path: &str, album: &str, artist: &str) -> ScanTrack {
        ScanTrack {
            album_title: Some(album.into()),
            artist_names: vec![artist.into()],
            album_artist_names: vec![artist.into()],
            ..scan_track(path, &format!("{album} {path}"))
        }
    }

    fn album_ids(lib: &SqliteLibrary) -> HashMap<String, i64> {
        lib.albums()
            .unwrap()
            .into_iter()
            .map(|a| (a.title, a.id))
            .collect()
    }

    fn artist_ids(lib: &SqliteLibrary) -> HashMap<String, i64> {
        lib.artists(ArtistGrouping::TrackArtist)
            .unwrap()
            .into_iter()
            .map(|a| (a.name, a.id))
            .collect()
    }

    #[test]
    fn artist_names_that_differ_only_in_case_are_one_artist() {
        let (lib, _) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/m"])).unwrap();
        scan(
            &lib,
            vec![
                on_album("/m/a.flac", "Paper Lanterns", "Glass the Harbor"),
                on_album("/m/b.flac", "Quiet Rooms", "Glass The Harbor"),
                on_album("/m/c.flac", "Мегаполис", "ЛИСЫ"),
                on_album("/m/d.flac", "Дворы", "Лисы"),
            ],
        );
        let artists = artist_ids(&lib);
        assert_eq!(artists.len(), 2);
        let glass = artists["Glass the Harbor"];
        assert_eq!(lib.upsert_artist("GLASS THE HARBOR").unwrap(), glass);

        scan(
            &lib,
            vec![
                on_album("/m/b.flac", "Quiet Rooms", "Glass The Harbor"),
                on_album("/m/a.flac", "Paper Lanterns", "Glass the Harbor"),
            ],
        );
        assert_eq!(
            artist_ids(&lib).into_values().collect::<Vec<_>>(),
            vec![glass]
        );
    }

    #[test]
    fn a_rescan_keeps_album_and_artist_ids_whatever_order_files_arrive_in() {
        let (lib, _) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/m"])).unwrap();
        scan(
            &lib,
            vec![
                on_album("/m/a.flac", "Night Drive", "Nova"),
                on_album("/m/b.flac", "Static", "KTX"),
            ],
        );
        let albums = album_ids(&lib);
        let artists = artist_ids(&lib);

        scan(
            &lib,
            vec![
                on_album("/m/c.flac", "Low Tide", "Mirrors"),
                on_album("/m/b.flac", "Static", "KTX"),
                on_album("/m/a.flac", "Night Drive", "Nova"),
            ],
        );
        let after = album_ids(&lib);
        assert_eq!(after["Night Drive"], albums["Night Drive"]);
        assert_eq!(after["Static"], albums["Static"]);
        assert!(!albums.values().any(|id| *id == after["Low Tide"]));
        let artists_after = artist_ids(&lib);
        assert_eq!(artists_after["Nova"], artists["Nova"]);
        assert_eq!(artists_after["KTX"], artists["KTX"]);

        let gone = after["Low Tide"];
        scan(
            &lib,
            vec![
                on_album("/m/a.flac", "Night Drive", "Nova"),
                on_album("/m/b.flac", "Static", "KTX"),
            ],
        );
        scan(
            &lib,
            vec![
                on_album("/m/a.flac", "Night Drive", "Nova"),
                on_album("/m/b.flac", "Static", "KTX"),
                on_album("/m/d.flac", "Second Sun", "Nova"),
            ],
        );
        assert!(album_ids(&lib)["Second Sun"] > gone);

        let edited = lib.upsert_album("Edited", Some(2001), None).unwrap();
        assert!(edited > album_ids(&lib)["Second Sun"]);

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_track(on_album("/m/a.flac", "Night Drive", "Nova"))
            .unwrap();
        session.flush().unwrap();
        let during = lib.upsert_album("During Scan", Some(2002), None).unwrap();
        session
            .add_track(on_album("/m/e.flac", "Afterglow", "Nova"))
            .unwrap();
        session.finish().unwrap();
        let ids = album_ids(&lib);
        assert_eq!(ids["Night Drive"], after["Night Drive"]);
        assert!(ids["Afterglow"] > during);
    }

    fn folders(paths: &[&str]) -> Vec<LocalFolder> {
        paths
            .iter()
            .map(|path| LocalFolder {
                path: (*path).into(),
                available: true,
            })
            .collect()
    }

    fn id_of(lib: &SqliteLibrary, path: &str) -> i64 {
        lib.all_tracks()
            .unwrap()
            .into_iter()
            .find(|t| t.path == path)
            .unwrap_or_else(|| panic!("{path} is not in the library"))
            .id
    }

    #[test]
    fn a_playlist_keeps_its_track_across_a_rescan_under_the_same_id() {
        let (lib, _path) = create_test_db();
        scan(&lib, vec![scan_track("/m/keep.flac", "Keep")]);
        let track_id = id_of(&lib, "/m/keep.flac");
        let playlist_id = lib.create_playlist("Keepers").unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();

        scan(&lib, vec![scan_track("/m/keep.flac", "Keep")]);

        assert_eq!(id_of(&lib, "/m/keep.flac"), track_id);
        let tracks = lib.tracks_for_playlist(playlist_id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, track_id);
        assert!(tracks[0].available);
    }

    #[test]
    fn a_track_missing_from_a_rescan_stays_in_its_playlist_and_comes_back() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                scan_track("/m/kept.flac", "Kept"),
                scan_track("/m/gone.flac", "Gone"),
            ],
        );
        let kept = id_of(&lib, "/m/kept.flac");
        let gone = id_of(&lib, "/m/gone.flac");
        let playlist_id = lib.create_playlist("Mixed").unwrap();
        lib.add_track_to_playlist(playlist_id, gone).unwrap();
        lib.add_track_to_playlist(playlist_id, kept).unwrap();

        scan(&lib, vec![scan_track("/m/kept.flac", "Kept")]);

        let tracks = lib.tracks_for_playlist(playlist_id).unwrap();
        let shape: Vec<(i64, &str, bool)> = tracks
            .iter()
            .map(|t| (t.id, t.title.as_str(), t.available))
            .collect();
        assert_eq!(shape, vec![(gone, "Gone", false), (kept, "Kept", true)]);
        assert_eq!(tracks[0].path, "/m/gone.flac");
        assert_eq!(tracks[0].duration_ms, Some(180_000));

        scan(
            &lib,
            vec![
                scan_track("/m/kept.flac", "Kept"),
                scan_track("/m/gone.flac", "Gone"),
            ],
        );
        assert_eq!(id_of(&lib, "/m/gone.flac"), gone);
        assert!(
            lib.tracks_for_playlist(playlist_id)
                .unwrap()
                .iter()
                .all(|t| t.available)
        );
    }

    #[test]
    fn fetched_lyrics_survive_a_rescan() {
        let (lib, _path) = create_test_db();
        scan(&lib, vec![scan_track("/m/fetched.flac", "Fetched")]);
        let track_id = id_of(&lib, "/m/fetched.flac");
        lib.upsert_lyrics(track_id, "la la la", "lrclib", false)
            .unwrap();

        scan(&lib, vec![scan_track("/m/fetched.flac", "Fetched")]);

        let kept = lyric(&lib, track_id, "lrclib").unwrap();
        assert_eq!(kept.text, "la la la");
    }

    #[test]
    fn clear_drops_disk_lyrics_but_keeps_fetched_ones() {
        let (lib, _path) = create_test_db();
        let lrc = seed_track(&lib, "Sidecar", "Album", "Artist");
        let embedded = seed_track(&lib, "Tagged", "Album", "Artist");
        let fetched = seed_track(&lib, "Fetched", "Album", "Artist");
        lib.upsert_lyrics(lrc, "from sidecar", "lrc", false)
            .unwrap();
        lib.upsert_lyrics(embedded, "from tag", "embedded", false)
            .unwrap();
        lib.upsert_lyrics(fetched, "from net", "lrclib", false)
            .unwrap();

        lib.clear().unwrap();

        assert!(lib.lyrics_variants(lrc).unwrap().is_empty());
        assert!(lib.lyrics_variants(embedded).unwrap().is_empty());
        assert_eq!(lyric(&lib, fetched, "lrclib").unwrap().text, "from net");
    }

    #[test]
    fn disk_and_fetched_lyrics_live_side_by_side_across_rescans() {
        let (lib, _path) = create_test_db();
        scan(&lib, vec![scan_track("/m/song.flac", "Song")]);
        let track_id = id_of(&lib, "/m/song.flac");
        lib.upsert_lyrics(track_id, "fetched", "lrclib", false)
            .unwrap();

        let mut with_both = scan_track("/m/song.flac", "Song");
        with_both.lyrics = vec![
            ScanLyrics {
                text: "from sidecar".into(),
                source: "lrc".into(),
            },
            ScanLyrics {
                text: "from tag".into(),
                source: "embedded".into(),
            },
        ];
        scan(&lib, vec![with_both]);

        let texts: Vec<(String, String)> = lib
            .lyrics_variants(track_id)
            .unwrap()
            .into_iter()
            .map(|v| (v.source, v.text))
            .collect();
        assert_eq!(
            texts,
            vec![
                ("lrc".to_string(), "from sidecar".to_string()),
                ("embedded".to_string(), "from tag".to_string()),
                ("lrclib".to_string(), "fetched".to_string()),
            ]
        );

        scan(&lib, vec![scan_track("/m/song.flac", "Song")]);
        let left: Vec<String> = lib
            .lyrics_variants(track_id)
            .unwrap()
            .into_iter()
            .map(|v| v.source)
            .collect();
        assert_eq!(left, vec!["lrclib".to_string()]);
    }

    fn stored(source: &str, text: &str, not_found: bool) -> StoredLyrics {
        StoredLyrics {
            source: source.into(),
            text: text.into(),
            not_found,
        }
    }

    #[test]
    fn lyrics_pick_follows_priority_and_choice() {
        let variants = vec![
            stored("lrclib", "net", false),
            stored("embedded", "tag", false),
            stored("lrc", "file", false),
        ];
        for (prefer_lrclib, chosen, expected) in [
            (false, None, "lrc"),
            (true, None, "lrclib"),
            (false, Some("embedded"), "embedded"),
            (false, Some("nope"), "lrc"),
        ] {
            assert_eq!(
                lyrics_source::pick(&variants, prefer_lrclib, chosen)
                    .unwrap()
                    .source,
                expected,
                "prefer_lrclib={prefer_lrclib} chosen={chosen:?}"
            );
        }
        let without_sidecar = &variants[..2];
        assert_eq!(
            lyrics_source::pick(without_sidecar, false, None)
                .unwrap()
                .source,
            "embedded"
        );
    }

    #[test]
    fn lyrics_choices_skip_not_found_and_duplicate_text() {
        let variants = vec![
            stored("lrclib", "same words\n", false),
            stored("embedded", "junk", false),
            stored("lrc", "same words", false),
        ];
        let sources: Vec<&str> = lyrics_source::choices(&variants, false)
            .iter()
            .map(|v| v.source.as_str())
            .collect();
        assert_eq!(sources, vec!["lrc", "embedded"]);

        let only_miss = vec![stored("lrclib", "", true), stored("embedded", "tag", false)];
        let sources: Vec<&str> = lyrics_source::choices(&only_miss, true)
            .iter()
            .map(|v| v.source.as_str())
            .collect();
        assert_eq!(sources, vec!["embedded"]);
    }

    #[test]
    fn a_rescan_sweeps_items_nothing_references() {
        let (lib, path) = create_test_db();
        scan(
            &lib,
            vec![
                scan_track("/m/liked.flac", "Liked"),
                scan_track("/m/plain.flac", "Plain"),
            ],
        );
        let kept = mark(&lib, id_of(&lib, "/m/liked.flac"));

        scan(&lib, vec![]);

        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 1);
        let kept_tracks = lib.tracks_for_playlist(kept).unwrap();
        assert_eq!(kept_tracks.len(), 1);
        assert_eq!(kept_tracks[0].title, "Liked");
        assert!(!kept_tracks[0].available);
    }

    #[test]
    fn a_swept_id_is_never_handed_to_another_track() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![scan_track("/m/a.flac", "A"), scan_track("/m/b.flac", "B")],
        );
        let swept = id_of(&lib, "/m/b.flac");

        scan(&lib, vec![scan_track("/m/a.flac", "A")]);
        scan(
            &lib,
            vec![scan_track("/m/a.flac", "A"), scan_track("/m/c.flac", "C")],
        );

        assert!(id_of(&lib, "/m/c.flac") > swept);
    }

    #[test]
    fn track_artists_map_has_no_parameter_ceiling_and_names_unavailable_tracks() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                scan_track("/m/kept.flac", "Kept"),
                scan_track("/m/gone.flac", "Gone"),
            ],
        );
        let kept = id_of(&lib, "/m/kept.flac");
        let gone = id_of(&lib, "/m/gone.flac");
        mark(&lib, gone);
        scan(&lib, vec![scan_track("/m/kept.flac", "Kept")]);

        let mut ids: Vec<i64> = (1_000_000..1_040_000).collect();
        ids.push(kept);
        ids.push(gone);
        let map = lib.track_artists_map(&ids).unwrap();

        assert_eq!(map.get(&kept), Some(&vec!["Artist".to_string()]));
        assert_eq!(map.get(&gone), Some(&vec!["Artist".to_string()]));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn the_guard_refuses_to_delete_an_item_that_carries_user_data() {
        let (lib, path) = create_test_db();
        scan(&lib, vec![scan_track("/m/liked.flac", "Liked")]);
        let track_id = id_of(&lib, "/m/liked.flac");
        mark(&lib, track_id);

        let conn = rusqlite::Connection::open(&path).unwrap();
        let err = conn
            .execute("DELETE FROM media_items WHERE id = ?1", [track_id])
            .unwrap_err();
        assert!(err.to_string().contains("referenced by user data"));
    }

    #[test]
    fn removing_a_folder_keeps_its_playlists_and_re_adding_it_revives_them() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let track_id = id_of(&lib, "/music/a.flac");
        let kept = mark(&lib, track_id);

        lib.reconcile_local_sources(&folders(&[])).unwrap();
        scan(&lib, vec![]);
        let hanging = lib.tracks_for_playlist(kept).unwrap();
        assert_eq!(hanging.len(), 1);
        assert!(!hanging[0].available);

        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        assert_eq!(id_of(&lib, "/music/a.flac"), track_id);
        let hanging = lib.tracks_for_playlist(kept).unwrap();
        assert_eq!(hanging.len(), 1);
        assert!(hanging[0].available);
    }

    fn untagged(path: &str, title: &str) -> ScanTrack {
        ScanTrack {
            path: path.into(),
            title: Some(title.into()),
            file_size: Some(content_size(title)),
            duration_ms: Some(120_000),
            ..Default::default()
        }
    }

    #[test]
    fn a_moved_file_keeps_its_id_and_playlist_entries() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/old/a.flac", "A")]);
        let track_id = id_of(&lib, "/music/old/a.flac");
        let playlist_id = lib.create_playlist("Mix").unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();

        scan(&lib, vec![scan_track("/music/new/a.flac", "A")]);

        assert_eq!(id_of(&lib, "/music/new/a.flac"), track_id);
        let playlist = lib.tracks_for_playlist(playlist_id).unwrap();
        assert_eq!(playlist.len(), 1);
        assert!(playlist[0].available);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 1);
        assert_eq!(
            count_rows(&path, "SELECT COUNT(*) FROM adoptions WHERE tier = 'file'"),
            1
        );
    }

    #[test]
    fn a_folder_re_added_from_a_new_mount_point_is_re_attached_by_relative_path() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/Volumes/Old"]))
            .unwrap();
        scan(&lib, vec![untagged("/Volumes/Old/x/01.flac", "01")]);
        let track_id = id_of(&lib, "/Volumes/Old/x/01.flac");
        let kept = mark(&lib, track_id);

        lib.reconcile_local_sources(&folders(&["/Volumes/New"]))
            .unwrap();
        scan(&lib, vec![untagged("/Volumes/New/x/01.flac", "01")]);

        assert_eq!(id_of(&lib, "/Volumes/New/x/01.flac"), track_id);
        assert!(lib.tracks_for_playlist(kept).unwrap()[0].available);
    }

    #[test]
    fn a_copy_next_to_its_original_is_a_separate_track() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let original = id_of(&lib, "/music/a.flac");
        mark(&lib, original);

        scan(
            &lib,
            vec![
                scan_track("/music/a.flac", "A"),
                scan_track("/music/copy/a.flac", "A"),
            ],
        );

        assert_eq!(id_of(&lib, "/music/a.flac"), original);
        assert_ne!(id_of(&lib, "/music/copy/a.flac"), original);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM adoptions"), 0);
    }

    #[test]
    fn a_copy_elsewhere_joins_the_track_of_an_offline_folder_which_is_never_swept() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/one.flac", "One"),
                scan_track("/b/two.flac", "Two"),
                scan_track("/b/plain.flac", "Plain"),
            ],
        );
        let two = id_of(&lib, "/b/two.flac");
        let plain = id_of(&lib, "/b/plain.flac");
        let kept = mark(&lib, two);

        lib.reconcile_local_sources(&[
            LocalFolder {
                path: "/a".into(),
                available: true,
            },
            LocalFolder {
                path: "/b".into(),
                available: false,
            },
        ])
        .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/one.flac", "One"),
                scan_track("/a/two-copy.flac", "Two"),
            ],
        );

        assert_eq!(id_of(&lib, "/a/two-copy.flac"), two);
        assert!(!lib.all_tracks().unwrap().iter().any(|t| t.id == plain));
        assert!(lib.tracks_for_playlist(kept).unwrap()[0].available);
        assert_eq!(
            count_rows(
                &path,
                "SELECT COUNT(*) FROM media_bindings WHERE present = 1 AND source_key LIKE '/b/%'"
            ),
            2
        );

        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/one.flac", "One"),
                scan_track("/a/two-copy.flac", "Two"),
                scan_track("/b/two.flac", "Two"),
                scan_track("/b/plain.flac", "Plain"),
            ],
        );
        assert_eq!(id_of(&lib, "/a/two-copy.flac"), two);
        assert_eq!(id_of(&lib, "/b/plain.flac"), plain);
        assert_eq!(paths(&lib).len(), 3);
        assert!(lib.tracks_for_playlist(kept).unwrap()[0].available);
    }

    #[test]
    fn a_copy_made_before_the_original_was_deleted_takes_over_its_like() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let original = id_of(&lib, "/music/a.flac");
        let playlist_id = lib.create_playlist("Mix").unwrap();
        lib.add_track_to_playlist(playlist_id, original).unwrap();
        let mut copy = scan_track("/music/copy/a.flac", "A");
        copy.lyrics = vec![ScanLyrics {
            text: "from disk".into(),
            source: "lrc".into(),
        }];
        scan(&lib, vec![scan_track("/music/a.flac", "A"), copy.clone()]);
        let copy_id = id_of(&lib, "/music/copy/a.flac");
        assert_ne!(copy_id, original);

        scan(&lib, vec![copy]);

        assert_eq!(id_of(&lib, "/music/copy/a.flac"), original);
        let kept_row = lib.tracks_for_playlist(playlist_id).unwrap();
        assert!(kept_row[0].available);
        assert_eq!(kept_row[0].path, "/music/copy/a.flac");
        assert!(lib.tracks_for_playlist(playlist_id).unwrap()[0].available);
        assert_eq!(
            lib.track_artists(original).unwrap(),
            vec!["Artist".to_string()]
        );
        assert_eq!(lyric(&lib, original, "lrc").unwrap().text, "from disk");
        assert_eq!(
            count_rows(
                &path,
                &format!("SELECT COUNT(*) FROM media_items WHERE id = {copy_id}")
            ),
            0
        );
        assert_eq!(
            count_rows(&path, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0
        );
    }

    #[test]
    fn a_moved_file_restored_to_its_old_path_stays_in_the_library_beside_the_move() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let original = id_of(&lib, "/music/a.flac");
        let playlist = lib.create_playlist("Mix").unwrap();
        lib.add_track_to_playlist(playlist, original).unwrap();
        scan(&lib, vec![scan_track("/music/b.flac", "A")]);
        assert_eq!(id_of(&lib, "/music/b.flac"), original);

        for _ in 0..2 {
            scan(
                &lib,
                vec![
                    scan_track("/music/a.flac", "A"),
                    scan_track("/music/b.flac", "A"),
                ],
            );
            let a = id_of(&lib, "/music/a.flac");
            let b = id_of(&lib, "/music/b.flac");
            assert_ne!(a, b);
            assert!(a == original || b == original);
            let kept_row = lib.tracks_for_playlist(playlist).unwrap();
            assert_eq!(kept_row.len(), 1);
            assert!(kept_row[0].available);
        }
    }

    #[test]
    fn the_same_file_in_two_folders_is_one_track_whatever_happens_to_either_folder() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![scan_track("/a/y.flac", "X"), scan_track("/b/x.flac", "X")],
        );
        assert_eq!(paths(&lib), vec!["/a/y.flac".to_string()]);
        let track_id = id_of(&lib, "/a/y.flac");
        let kept = mark(&lib, track_id);

        lib.reconcile_local_sources(&folders(&["/b"])).unwrap();
        scan(&lib, vec![scan_track("/b/x.flac", "X")]);
        assert_eq!(id_of(&lib, "/b/x.flac"), track_id);

        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/y.flac", "X"),
                scan_track("/a/new.flac", "New"),
                scan_track("/b/x.flac", "X"),
            ],
        );
        assert_eq!(
            paths(&lib),
            vec!["/a/new.flac".to_string(), "/a/y.flac".to_string()]
        );
        assert_eq!(id_of(&lib, "/a/y.flac"), track_id);
        let kept_row = lib.tracks_for_playlist(kept).unwrap();
        assert_eq!(kept_row.len(), 1);
        assert!(kept_row[0].available);
    }

    #[test]
    fn a_revived_like_keeps_its_fetched_lyrics_when_the_file_has_none() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let original = id_of(&lib, "/music/a.flac");
        mark(&lib, original);
        lib.upsert_lyrics(original, "from the net", "lrclib", false)
            .unwrap();
        let copy = scan_track("/music/copy/a.flac", "A");
        scan(&lib, vec![scan_track("/music/a.flac", "A"), copy.clone()]);
        scan(
            &lib,
            vec![copy.clone(), scan_track("/music/new.flac", "New")],
        );

        assert_eq!(id_of(&lib, "/music/copy/a.flac"), original);
        let lyrics = lyric(&lib, original, "lrclib").unwrap();
        assert_eq!(lyrics.text, "from the net");
    }

    #[test]
    fn sources_report_availability_and_the_tracks_each_folder_contributes() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/1.flac", "One"),
                scan_track("/a/2.flac", "Two"),
                scan_track("/b/3.flac", "Three"),
            ],
        );
        let shape = |lib: &SqliteLibrary| -> Vec<(String, bool, bool, i64)> {
            lib.sources()
                .unwrap()
                .into_iter()
                .map(|s| (s.uri, s.enabled, s.available, s.track_count))
                .collect()
        };
        assert_eq!(
            shape(&lib),
            vec![("/a".into(), true, true, 2), ("/b".into(), true, true, 1)]
        );

        lib.reconcile_local_sources(&[
            LocalFolder {
                path: "/a".into(),
                available: true,
            },
            LocalFolder {
                path: "/b".into(),
                available: false,
            },
        ])
        .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/1.flac", "One"),
                scan_track("/a/2.flac", "Two"),
            ],
        );
        assert_eq!(
            shape(&lib),
            vec![("/a".into(), true, true, 2), ("/b".into(), true, false, 1)]
        );

        lib.reconcile_local_sources(&folders(&["/a"])).unwrap();
        assert!(!shape(&lib)[1].1);
    }

    #[test]
    fn a_scan_holds_the_write_lock_only_until_it_catches_up() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let other = rusqlite::Connection::open(&path).unwrap();
        other.busy_timeout(std::time::Duration::ZERO).unwrap();
        let can_write = || other.execute_batch("BEGIN IMMEDIATE; COMMIT").is_ok();

        let mut session = lib.open_scan_session().unwrap();
        assert!(can_write());
        session.clear().unwrap();
        assert!(!can_write());
        session.flush().unwrap();
        assert!(can_write());
        session
            .add_track(scan_track("/music/new.flac", "New"))
            .unwrap();
        assert!(can_write());
        session.add_track(scan_track("/music/a.flac", "A")).unwrap();
        assert!(!can_write());
        session.flush().unwrap();
        assert!(can_write());
        session.finish().unwrap();
        assert!(can_write());
        assert_eq!(paths(&lib).len(), 2);
    }

    #[test]
    fn a_file_listed_twice_in_one_scan_keeps_its_id_and_playlist_entry() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music", "/music/rock"]))
            .unwrap();
        let track = untagged("/music/rock/a.flac", "A");
        scan(&lib, vec![track.clone(), track.clone()]);
        let track_id = id_of(&lib, "/music/rock/a.flac");
        let playlist_id = lib.create_playlist("Mix").unwrap();
        lib.add_track_to_playlist(playlist_id, track_id).unwrap();

        for _ in 0..3 {
            scan(&lib, vec![track.clone(), track.clone()]);
        }

        assert_eq!(id_of(&lib, "/music/rock/a.flac"), track_id);
        assert_eq!(
            lib.playback_locators(track_id).unwrap(),
            vec![("/music/rock/a.flac".to_string(), 0)]
        );
        let playlist = lib.tracks_for_playlist(playlist_id).unwrap();
        assert_eq!(playlist.len(), 1);
        assert!(playlist[0].available);
    }

    #[test]
    fn a_playlist_edit_waits_for_the_scan_batch_instead_of_failing() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let track_id = id_of(&lib, "/music/a.flac");
        let playlist_id = lib.create_playlist("Mix").unwrap();
        let lib = std::sync::Arc::new(lib);
        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();

        let writer = lib.clone();
        let edits = std::thread::spawn(move || {
            (
                writer.add_track_to_playlist(playlist_id, track_id).is_ok(),
                writer
                    .add_tracks_to_playlist(playlist_id, &[track_id])
                    .is_ok(),
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        session.flush().unwrap();

        assert_eq!(edits.join().unwrap(), (true, true));
        drop(session);
    }

    #[test]
    fn a_migrated_file_that_never_shows_up_again_stops_counting_as_unplaced() {
        let path = fresh_db_path();
        build_v8_db(
            &path,
            &format!(
                "{V8_CATALOG}
                INSERT INTO tracks (id, path, title, duration_ms, start_offset_ms, liked, is_cue)
                    VALUES (13, '/m/deleted.flac', 'Deleted', 1000, 0, 1, 0),
                           (14, '/offline/kept.flac', 'Kept', 1000, 0, 1, 0);"
            ),
        );
        let lib = SqliteLibrary::open_at(&path).unwrap();
        lib.reconcile_local_sources(&[online("/m"), offline("/offline")])
            .unwrap();
        let mut cue_b = scan_track("/m/cue.flac", "Cue B");
        cue_b.start_offset_ms = Some(2000);
        scan(
            &lib,
            vec![
                scan_track("/m/one.flac", "One"),
                scan_track("/m/cue.flac", "Cue A"),
                cue_b,
            ],
        );
        assert!(lib.has_unplaced_media().unwrap());

        lib.reconcile_local_sources(&[online("/m")]).unwrap();
        scan(
            &lib,
            vec![
                scan_track("/m/one.flac", "One"),
                scan_track("/m/cue.flac", "Cue A"),
            ],
        );
        assert!(!lib.has_unplaced_media().unwrap());
        let seeded = imported_likes_playlist(&path);
        let liked: Vec<i64> = lib
            .tracks_for_playlist(seeded)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert!(liked.contains(&13) && liked.contains(&14));
    }

    #[test]
    fn the_catalog_row_comes_from_the_file_in_the_first_folder_whatever_the_scan_order() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        let mut first = scan_track("/a/x.flac", "X");
        first.year = Some(2000);
        let mut second = scan_track("/b/x.flac", "X");
        second.year = Some(1999);
        second.is_cue = true;
        second.start_offset_ms = Some(0);
        for order in [
            vec![first.clone(), second.clone()],
            vec![second.clone(), first.clone()],
        ] {
            scan(&lib, order);
            let tracks = lib.all_tracks().unwrap();
            assert_eq!(tracks.len(), 1);
            assert_eq!(tracks[0].path, "/a/x.flac");
            assert_eq!(tracks[0].year, Some(2000));
            assert!(!tracks[0].is_cue);
        }
    }

    #[test]
    fn a_file_restored_beside_its_move_splits_off_even_when_another_folder_holds_the_track() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![scan_track("/a/s.flac", "S"), scan_track("/b/s.flac", "S")],
        );
        let id = id_of(&lib, "/a/s.flac");
        scan(
            &lib,
            vec![scan_track("/a/s.flac", "S"), scan_track("/b/s2.flac", "S")],
        );
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 1);

        scan(
            &lib,
            vec![
                scan_track("/a/s.flac", "S"),
                scan_track("/b/s2.flac", "S"),
                scan_track("/b/s.flac", "S"),
            ],
        );
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 2);
        assert_eq!(
            count_rows(
                &path,
                &format!(
                    "SELECT COUNT(*) FROM media_bindings b JOIN sources s ON s.id = b.source_id \
                     WHERE b.item_id = {id} AND b.present = 1 AND s.uri = '/b'"
                )
            ),
            1
        );
        assert_eq!(paths(&lib).len(), 2);
    }

    #[test]
    fn a_retagged_copy_in_another_folder_joins_by_tags_when_its_size_differs() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a"])).unwrap();
        scan(&lib, vec![scan_track("/a/x.flac", "X")]);
        let id = id_of(&lib, "/a/x.flac");
        lib.reconcile_local_sources(&[offline("/a"), online("/b")])
            .unwrap();
        let mut copy = scan_track("/b/x.flac", "X");
        copy.file_size = Some(1);
        copy.duration_ms = Some(181_000);
        scan(&lib, vec![copy]);
        assert_eq!(id_of(&lib, "/b/x.flac"), id);
        assert_eq!(
            count_rows(&path, "SELECT COUNT(*) FROM adoptions WHERE tier = 'tags'"),
            1
        );
    }

    fn offline(path: &str) -> LocalFolder {
        LocalFolder {
            path: path.into(),
            available: false,
        }
    }

    fn online(path: &str) -> LocalFolder {
        LocalFolder {
            path: path.into(),
            available: true,
        }
    }

    #[test]
    fn a_folder_renamed_on_disk_and_added_again_while_the_old_path_stays_configured_keeps_everything()
     {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        let before = vec![
            scan_track("/music/x/a.flac", "A"),
            scan_track("/music/x/b.flac", "B"),
            scan_track("/music/x/c.flac", "C"),
            untagged("/music/loose/01.flac", "01"),
        ];
        scan(&lib, before);
        let a = id_of(&lib, "/music/x/a.flac");
        let b = id_of(&lib, "/music/x/b.flac");
        let c = id_of(&lib, "/music/x/c.flac");
        let loose = id_of(&lib, "/music/loose/01.flac");
        let kept = mark(&lib, a);
        mark(&lib, loose);
        let playlist = lib.create_playlist("Mix").unwrap();
        lib.add_track_to_playlist(playlist, b).unwrap();

        lib.reconcile_local_sources(&[offline("/music"), online("/music 2")])
            .unwrap();
        let after = vec![
            scan_track("/music 2/x/a.flac", "A"),
            scan_track("/music 2/x/b.flac", "B"),
            scan_track("/music 2/x/c.flac", "C"),
            untagged("/music 2/loose/01.flac", "01"),
        ];
        scan(&lib, after.clone());

        assert_eq!(id_of(&lib, "/music 2/x/a.flac"), a);
        assert_eq!(id_of(&lib, "/music 2/x/b.flac"), b);
        assert_eq!(id_of(&lib, "/music 2/x/c.flac"), c);
        assert_eq!(id_of(&lib, "/music 2/loose/01.flac"), loose);
        assert_eq!(paths(&lib).len(), 4);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 4);
        assert!(lib.tracks_for_playlist(kept).unwrap().iter().all(|t| t.available));
        assert!(lib.tracks_for_playlist(playlist).unwrap()[0].available);

        lib.reconcile_local_sources(&folders(&["/music 2"]))
            .unwrap();
        scan(&lib, after);
        assert_eq!(paths(&lib).len(), 4);
        assert_eq!(id_of(&lib, "/music 2/x/a.flac"), a);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 4);
    }

    #[test]
    fn folders_added_together_to_an_existing_library_all_join_the_same_track() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a"])).unwrap();
        scan(&lib, vec![scan_track("/a/x.flac", "X")]);
        let x = id_of(&lib, "/a/x.flac");

        lib.reconcile_local_sources(&folders(&["/a", "/b", "/c"]))
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/x.flac", "X"),
                scan_track("/b/x.flac", "X"),
                scan_track("/c/copy of x.flac", "X"),
            ],
        );
        assert_eq!(paths(&lib), vec!["/a/x.flac".to_string()]);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 1);
        assert_eq!(
            count_rows(
                &path,
                &format!("SELECT COUNT(*) FROM media_bindings WHERE item_id = {x} AND present = 1")
            ),
            3
        );

        lib.reconcile_local_sources(&[offline("/a"), online("/b"), online("/c")])
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/b/x.flac", "X"),
                scan_track("/c/copy of x.flac", "X"),
            ],
        );
        assert_eq!(paths(&lib), vec!["/b/x.flac".to_string()]);
        assert_eq!(id_of(&lib, "/b/x.flac"), x);
    }

    #[test]
    fn copies_inside_one_folder_stay_separate_tracks() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(
            &lib,
            vec![
                scan_track("/a/x.flac", "X"),
                scan_track("/a/x copy.flac", "X"),
                scan_track("/b/x.flac", "X"),
            ],
        );
        assert_eq!(paths(&lib).len(), 2);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 2);
        scan(
            &lib,
            vec![
                scan_track("/a/x.flac", "X"),
                scan_track("/a/x copy.flac", "X"),
                scan_track("/b/x.flac", "X"),
            ],
        );
        assert_eq!(paths(&lib).len(), 2);
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 2);
    }

    #[test]
    fn cue_tracks_of_one_image_in_two_folders_join_track_by_track() {
        let (lib, path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        let cue = |root: &str, offset: u64, title: &str| {
            let mut track = untagged(&format!("{root}/image.ape"), title);
            track.start_offset_ms = Some(offset);
            track.is_cue = true;
            track.file_size = Some(777_000);
            track.duration_ms = Some(if offset == 0 { 60_000 } else { 90_000 });
            track
        };
        scan(
            &lib,
            vec![
                cue("/a", 0, "One"),
                cue("/a", 60_000, "Two"),
                cue("/b", 0, "Uno"),
                cue("/b", 60_000, "Dos"),
            ],
        );
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM media_items"), 2);
        assert_eq!(paths(&lib).len(), 2);
    }

    fn id_of_title(lib: &SqliteLibrary, title: &str) -> i64 {
        lib.all_tracks()
            .unwrap()
            .into_iter()
            .find(|t| t.title == title)
            .unwrap_or_else(|| panic!("{title} is not in the library"))
            .id
    }

    #[test]
    fn removing_a_folder_hands_its_kept_track_to_a_copy_in_another_folder() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        let mut other = scan_track("/b/a.mp3", "A");
        other.album_title = Some("Best Of".into());
        other.file_size = Some(1);
        scan(&lib, vec![scan_track("/a/a.flac", "A"), other.clone()]);
        let track_id = id_of(&lib, "/a/a.flac");
        let kept = mark(&lib, track_id);
        assert_eq!(paths(&lib).len(), 2);

        lib.reconcile_local_sources(&folders(&["/b"])).unwrap();
        scan(&lib, vec![other]);

        assert_eq!(id_of(&lib, "/b/a.mp3"), track_id);
        assert!(lib.tracks_for_playlist(kept).unwrap()[0].available);
    }

    #[test]
    fn an_offline_track_is_not_merged_with_another_recording_on_title_alone() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/a", "/b"]))
            .unwrap();
        scan(&lib, vec![scan_track("/a/a.flac", "A")]);
        let kept = mark(&lib, id_of(&lib, "/a/a.flac"));

        lib.reconcile_local_sources(&[offline("/a"), online("/b")])
            .unwrap();
        let mut live = scan_track("/b/a-live.flac", "A");
        live.album_title = Some("Live".into());
        live.file_size = Some(1);
        scan(&lib, vec![live]);

        assert_eq!(lib.tracks_for_playlist(kept).unwrap().len(), 1);
        assert!(!lib.tracks_for_playlist(kept).unwrap()[0].available);
    }

    fn paths(lib: &SqliteLibrary) -> Vec<String> {
        let mut paths: Vec<String> = lib
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn a_kept_row_moves_to_its_copy_on_the_scan_where_the_original_disappears() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let original = id_of(&lib, "/music/a.flac");
        let kept = mark(&lib, original);
        let copy = scan_track("/music/copy/a.flac", "A");
        scan(&lib, vec![scan_track("/music/a.flac", "A"), copy.clone()]);
        assert_ne!(id_of(&lib, "/music/copy/a.flac"), original);

        scan(&lib, vec![copy]);

        assert_eq!(id_of(&lib, "/music/copy/a.flac"), original);
        assert!(lib.tracks_for_playlist(kept).unwrap()[0].available);
    }

    #[test]
    fn a_deleted_file_is_adopted_by_its_retagged_replacement_only_within_tolerance() {
        let (lib, _path) = create_test_db();
        lib.reconcile_local_sources(&folders(&["/music"])).unwrap();
        scan(&lib, vec![scan_track("/music/a.flac", "A")]);
        let track_id = id_of(&lib, "/music/a.flac");
        let kept = mark(&lib, track_id);

        let mut other_length = scan_track("/music/a-live.flac", "A");
        other_length.album_title = Some("Live".into());
        other_length.duration_ms = Some(240_000);
        scan(&lib, vec![other_length]);

        assert_ne!(id_of(&lib, "/music/a-live.flac"), track_id);
        assert!(!lib.tracks_for_playlist(kept).unwrap()[0].available);
    }

    #[test]
    fn test_scan_session_buffers_track_until_cover_arrives() {
        // A track may reach the writer before its cover thumbnail; it must be
        // buffered and linked once add_cover lands.
        let (lib, _path) = create_test_db();
        let cover = make_test_jpeg(&[10, 20, 30]);
        let hash = sha256_hex(&cover);
        let thumbs = crate::thumbnail::generate_thumbnails(&cover).unwrap();

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_track(ScanTrack {
                is_cue: false,
                path: "/music/a.flac".into(),
                title: Some("A".into()),
                album_title: Some("Album".into()),
                artist_names: vec!["Artist".into()],
                album_artist_names: vec!["Artist".into()],
                track_number: Some(1),
                disc_number: Some(1),
                year: Some(2020),
                genres: vec![],
                duration_ms: Some(1000),
                cover_hash: Some(hash.clone()),
                start_offset_ms: None,
                bitrate: None,
                lyrics: Vec::new(),
                file_size: None,
            })
            .unwrap();
        session
            .add_cover(&hash, thumbs.small, thumbs.large, "/music/a.flac", true)
            .unwrap();
        session.finish().unwrap();
        // The album's cover and artists are settled after the scan, not during
        // it — see `resolve_album_covers` / `resolve_album_artists`.
        lib.resolve_album_covers().unwrap();
        lib.resolve_album_artists().unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Album");
        assert_eq!(albums[0].artist_name, "Artist");
        assert!(albums[0].cover_art_id.is_some());
        let tracks = lib.tracks_for_album(albums[0].id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "A");
        assert_eq!(tracks[0].cover_art_id, albums[0].cover_art_id);
        assert_eq!(
            lib.track_album_artists(tracks[0].id).unwrap(),
            vec!["Artist".to_string()]
        );
    }

    #[test]
    fn test_cover_art_source_roundtrip() {
        let (lib, _path) = create_test_db();
        let cover = make_test_jpeg(&[7, 8, 9]);
        let hash = sha256_hex(&cover);
        let thumbs = crate::thumbnail::generate_thumbnails(&cover).unwrap();

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_cover(&hash, thumbs.small, thumbs.large, "/m/art/cover.jpg", false)
            .unwrap();
        session.finish().unwrap();

        let (_, id) = lib
            .cover_art_hashes()
            .unwrap()
            .into_iter()
            .find(|(h, _)| *h == hash)
            .unwrap();
        assert_eq!(
            lib.get_cover_art_source(id).unwrap(),
            Some(("/m/art/cover.jpg".to_string(), false))
        );
    }

    /// A cover saved outside a scan records its origin just like one saved during a
    /// scan, so the "open the original" paths reach the full-size image instead of
    /// falling back to re-reading the track. Rows left over from the migration that
    /// added these columns still have no source; that fallback lives in
    /// `music_indexer::metadata::load_cover_from_source`.
    #[test]
    fn test_save_cover_art_records_where_it_came_from() {
        let (lib, _path) = create_test_db();
        let embedded = lib
            .save_cover_art(&make_test_jpeg(&[4, 5, 6]), "/music/a.flac", true)
            .unwrap();
        let external = lib
            .save_cover_art(&make_test_jpeg(&[7, 8, 9]), "/music/cover.jpg", false)
            .unwrap();

        assert_eq!(
            lib.get_cover_art_source(embedded).unwrap(),
            Some(("/music/a.flac".to_string(), true))
        );
        assert_eq!(
            lib.get_cover_art_source(external).unwrap(),
            Some(("/music/cover.jpg".to_string(), false))
        );
    }

    #[test]
    fn test_add_cover_keeps_first_source_across_rescans() {
        let (lib, _path) = create_test_db();
        let cover = make_test_jpeg(&[11, 12, 13]);
        let hash = sha256_hex(&cover);
        let thumbs = crate::thumbnail::generate_thumbnails(&cover).unwrap();

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_cover(
                &hash,
                thumbs.small.clone(),
                thumbs.large.clone(),
                "/first/cover.jpg",
                false,
            )
            .unwrap();
        session.finish().unwrap();

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_cover(&hash, thumbs.small, thumbs.large, "/second/cover.jpg", true)
            .unwrap();
        session.finish().unwrap();

        let (_, id) = lib
            .cover_art_hashes()
            .unwrap()
            .into_iter()
            .find(|(h, _)| *h == hash)
            .unwrap();
        assert_eq!(
            lib.get_cover_art_source(id).unwrap(),
            Some(("/first/cover.jpg".to_string(), false))
        );
    }

    #[test]
    fn test_scan_session_reuses_existing_cover_without_add_cover() {
        // A cover already in the DB (survives clear) resolves by hash from the
        // seeded cache — no add_cover needed, no duplicate row.
        let (lib, _path) = create_test_db();
        let cover = make_test_jpeg(&[1, 2, 3]);
        let existing_id = lib.save_cover_art(&cover, "/music/a.flac", true).unwrap();
        let hash = sha256_hex(&cover);

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_track(ScanTrack {
                is_cue: false,
                path: "/m/x.flac".into(),
                title: Some("X".into()),
                album_title: Some("Al".into()),
                artist_names: vec!["Ar".into()],
                album_artist_names: vec![],
                track_number: Some(1),
                disc_number: Some(1),
                year: None,
                genres: vec![],
                duration_ms: None,
                cover_hash: Some(hash),
                start_offset_ms: None,
                bitrate: None,
                lyrics: Vec::new(),
                file_size: None,
            })
            .unwrap();
        session.finish().unwrap();

        let albums = lib.albums().unwrap();
        let tracks = lib.tracks_for_album(albums[0].id).unwrap();
        assert_eq!(tracks[0].cover_art_id, Some(existing_id));
    }

    #[test]
    fn test_scan_session_commits_across_batch_boundary() {
        // Exercise the COMMIT/BEGIN cycle: insert well over SCAN_BATCH_SIZE.
        let (lib, _path) = create_test_db();
        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        for i in 0..600 {
            session
                .add_track(ScanTrack {
                    is_cue: false,
                    path: format!("/m/{i}.flac"),
                    title: Some(format!("T{i}")),
                    album_title: Some("Big".into()),
                    artist_names: vec!["Ar".into()],
                    album_artist_names: vec!["Ar".into()],
                    track_number: Some(i as u32),
                    disc_number: Some(1),
                    year: Some(2020),
                    genres: vec![],
                    duration_ms: Some(1000),
                    cover_hash: None,
                    start_offset_ms: None,
                    bitrate: None,
                    lyrics: Vec::new(),
                    file_size: None,
                })
                .unwrap();
        }
        session.finish().unwrap();

        let albums = lib.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(lib.tracks_for_album(albums[0].id).unwrap().len(), 600);
    }

    #[test]
    fn test_scan_session_writes_lyrics() {
        let (lib, _path) = create_test_db();
        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_track(ScanTrack {
                is_cue: false,
                path: "/m/lyric.flac".into(),
                title: Some("Lyric".into()),
                album_title: Some("Al".into()),
                artist_names: vec!["Ar".into()],
                album_artist_names: vec!["Ar".into()],
                track_number: Some(1),
                disc_number: Some(1),
                year: Some(2020),
                genres: vec![],
                duration_ms: Some(1000),
                cover_hash: None,
                start_offset_ms: None,
                bitrate: None,
                lyrics: vec![ScanLyrics {
                    text: "[00:01.00] hello\n[00:02.00] world".into(),
                    source: "lrclib".into(),
                }],
                file_size: None,
            })
            .unwrap();
        session.finish().unwrap();

        let tracks = lib.all_tracks().unwrap();
        assert_eq!(tracks.len(), 1);
        let stored = lyric(&lib, tracks[0].id, "lrclib").unwrap();
        assert!(!stored.not_found);
        assert_eq!(stored.text, "[00:01.00] hello\n[00:02.00] world");
    }

    #[test]
    fn test_lyrics_cascade_on_clear() {
        let (lib, _path) = create_test_db();
        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_track(ScanTrack {
                is_cue: false,
                path: "/m/c.flac".into(),
                title: Some("C".into()),
                album_title: Some("Al".into()),
                artist_names: vec!["Ar".into()],
                album_artist_names: vec!["Ar".into()],
                track_number: Some(1),
                disc_number: Some(1),
                year: Some(2020),
                genres: vec![],
                duration_ms: Some(1000),
                cover_hash: None,
                start_offset_ms: None,
                bitrate: None,
                lyrics: vec![ScanLyrics {
                    text: "to be wiped".into(),
                    source: "embedded".into(),
                }],
                file_size: None,
            })
            .unwrap();
        session.finish().unwrap();

        let track_id = lib.all_tracks().unwrap()[0].id;
        assert!(lyric(&lib, track_id, "embedded").is_some());

        lib.clear().unwrap();
        assert!(lib.lyrics_variants(track_id).unwrap().is_empty());
    }

    #[test]
    fn test_scan_meta_roundtrip() {
        let (lib, _path) = create_test_db();
        assert!(lib.scan_fingerprint().unwrap().is_none());
        assert!(lib.scan_folders().unwrap().is_none());
        lib.set_scan_meta("fp123", "/a\n/b").unwrap();
        assert_eq!(lib.scan_fingerprint().unwrap(), Some("fp123".into()));
        assert_eq!(lib.scan_folders().unwrap(), Some("/a\n/b".into()));
        lib.set_scan_meta("fp456", "/c").unwrap();
        assert_eq!(lib.scan_fingerprint().unwrap(), Some("fp456".into()));
        assert_eq!(lib.scan_folders().unwrap(), Some("/c".into()));
    }

    #[test]
    fn test_artist_album_covers_oldest_first_capped_at_three() {
        let (lib, _path) = create_test_db();

        let cover1 = lib
            .save_cover_art(&make_test_jpeg(&[255, 0, 0]), "/music/a.flac", true)
            .unwrap();
        let cover2 = lib
            .save_cover_art(&make_test_jpeg(&[0, 255, 0]), "/music/a.flac", true)
            .unwrap();
        let cover3 = lib
            .save_cover_art(&make_test_jpeg(&[0, 0, 255]), "/music/a.flac", true)
            .unwrap();
        let cover4 = lib
            .save_cover_art(&make_test_jpeg(&[128, 128, 0]), "/music/a.flac", true)
            .unwrap();

        let artist = lib.upsert_artist("Radiohead").unwrap();
        let other = lib.upsert_artist("Other").unwrap();

        let mk_album = |title: &str, year: Option<i32>, cover: Option<i64>| {
            let id = lib.upsert_album(title, year, cover).unwrap();
            lib.set_album_artists(id, &[(artist, 0)]).unwrap();
            id
        };
        let album1990 = mk_album("Old Album", Some(1990), Some(cover1));
        let album2000 = mk_album("Mid Album", Some(2000), Some(cover2));
        let album2010 = mk_album("New Album", Some(2010), Some(cover3));
        let album2020 = mk_album("Newest Album", Some(2020), Some(cover4));

        let mk_track = |path: &str, _album_id: i64| NewTrack {
            path: path.into(),
            title: Some("T".into()),
            album_title: None,
            artist_names: vec![],
            track_number: Some(1),
            disc_number: Some(1),
            year: None,
            duration_ms: Some(120_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(
            &mk_track("/p/t1.flac", album1990),
            Some(album1990),
            &[(artist, 0)],
        )
        .unwrap();
        lib.upsert_track(
            &mk_track("/p/t2.flac", album2000),
            Some(album2000),
            &[(artist, 0)],
        )
        .unwrap();
        lib.upsert_track(
            &mk_track("/p/t3.flac", album2010),
            Some(album2010),
            &[(artist, 0)],
        )
        .unwrap();
        lib.upsert_track(
            &mk_track("/p/t4.flac", album2020),
            Some(album2020),
            &[(artist, 0)],
        )
        .unwrap();

        // Other artist — should not bleed into result.
        let other_album = lib
            .upsert_album("Other Album", Some(2005), Some(cover2))
            .unwrap();
        lib.set_album_artists(other_album, &[(other, 0)]).unwrap();
        lib.upsert_track(
            &mk_track("/p/t5.flac", other_album),
            Some(other_album),
            &[(other, 0)],
        )
        .unwrap();

        let covers = lib
            .artist_album_covers(ArtistGrouping::TrackArtist)
            .unwrap();

        let artist_covers = covers.get(&artist).unwrap();
        // At most 3, oldest-first: cover1 (1990), cover2 (2000), cover3 (2010).
        assert_eq!(artist_covers.len(), 3);
        assert_eq!(artist_covers[0], cover1);
        assert_eq!(artist_covers[1], cover2);
        assert_eq!(artist_covers[2], cover3);

        // Other artist is present separately.
        assert!(covers.contains_key(&other));
        // Artist without covers is absent.
        let no_cover_artist = lib.upsert_artist("Silent").unwrap();
        let bare_album = lib.upsert_album("Bare", Some(2000), None).unwrap();
        lib.upsert_track(
            &mk_track("/p/t6.flac", bare_album),
            Some(bare_album),
            &[(no_cover_artist, 0)],
        )
        .unwrap();
        let covers2 = lib
            .artist_album_covers(ArtistGrouping::TrackArtist)
            .unwrap();
        assert!(!covers2.contains_key(&no_cover_artist));
    }

    fn sorted_album_genres(lib: &SqliteLibrary, album_id: i64) -> Vec<String> {
        let mut genres = lib.album_genres(album_id).unwrap();
        genres.sort();
        genres
    }

    #[test]
    fn test_album_artists_in_position_order() {
        let (lib, _path) = create_test_db();
        let second = lib.upsert_artist("Bravo").unwrap();
        let first = lib.upsert_artist("Alpha").unwrap();
        let album_id = lib.upsert_album("Split", Some(2001), None).unwrap();
        lib.set_album_artists(album_id, &[(second, 1), (first, 0)])
            .unwrap();

        assert_eq!(
            lib.album_artists(album_id).unwrap(),
            vec!["Alpha".to_string(), "Bravo".to_string()]
        );
        assert!(lib.album_artists(99_999).unwrap().is_empty());
    }

    #[test]
    fn test_set_track_genres_replaces_links() {
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();

        lib.set_track_genres(track_id, &["Ambient".into(), "Techno".into()])
            .unwrap();
        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Ambient".to_string(), "Techno".to_string()]
        );

        lib.set_track_genres(track_id, &["Jazz".into()]).unwrap();
        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Jazz".to_string()],
            "old genre links must be gone, not merged"
        );

        lib.set_track_genres(track_id, &[]).unwrap();
        assert!(sorted_album_genres(&lib, album_id).is_empty());
    }

    #[test]
    fn test_set_track_genres_dedups_case_insensitively() {
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();

        lib.set_track_genres(
            track_id,
            &["Ambient".into(), "ambient".into(), "  Techno  ".into()],
        )
        .unwrap();

        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Ambient".to_string(), "Techno".to_string()]
        );
    }

    #[test]
    fn test_set_track_genres_drops_orphaned_genres() {
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");

        lib.set_track_genres(track_id, &["Ambient".into()]).unwrap();
        lib.set_track_genres(track_id, &["Techno".into()]).unwrap();

        let orphans = lib
            .album_genres_map()
            .unwrap()
            .into_values()
            .flatten()
            .filter(|g| g == "Ambient")
            .count();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn test_upsert_track_preserves_id_is_cue_and_cover() {
        let (lib, _path) = create_test_db();
        let jpeg = make_test_jpeg(&[10, 20, 30]);

        let mut session = lib.open_scan_session().unwrap();
        session.clear().unwrap();
        session
            .add_cover("cover-hash", jpeg.clone(), jpeg, "/m/cover.jpg", false)
            .unwrap();
        session
            .add_track(ScanTrack {
                is_cue: true,
                path: "/m/whole_album.flac".into(),
                title: Some("Cue Track".into()),
                album_title: Some("Cue Album".into()),
                artist_names: vec!["Old Artist".into()],
                album_artist_names: vec!["Old Artist".into()],
                track_number: Some(1),
                disc_number: Some(1),
                year: Some(1999),
                genres: vec!["Ambient".into()],
                duration_ms: Some(1000),
                cover_hash: Some("cover-hash".into()),
                start_offset_ms: Some(5000),
                bitrate: None,
                lyrics: Vec::new(),
                file_size: None,
            })
            .unwrap();
        session.finish().unwrap();

        let before = lib.all_tracks().unwrap().remove(0);
        let cover_id = before.cover_art_id.expect("scan attached the cover");

        let album_id = lib.upsert_album("Cue Album", Some(1999), None).unwrap();
        let new_artist = lib.upsert_artist("New Artist").unwrap();
        let updated = NewTrack {
            path: "/m/whole_album.flac".into(),
            title: Some("Renamed".into()),
            album_title: Some("Cue Album".into()),
            artist_names: vec!["New Artist".into()],
            track_number: Some(4),
            disc_number: Some(2),
            year: Some(2001),
            duration_ms: Some(1000),
            cover_art_id: None,
            start_offset_ms: Some(5000),
            bitrate: None,
        };
        let same_id = lib
            .upsert_track(&updated, Some(album_id), &[(new_artist, 0)])
            .unwrap();

        assert_eq!(same_id, before.id, "content key must keep the row id");

        let after = lib.track(before.id).unwrap().unwrap();
        assert_eq!(after.title, "Renamed");
        assert_eq!(after.track_number, Some(4));
        assert_eq!(after.disc_number, 2);
        assert_eq!(after.year, Some(2001));
        assert!(after.is_cue, "is_cue must survive a tag edit");
        assert_eq!(
            after.cover_art_id,
            Some(cover_id),
            "a None cover must not wipe the existing one"
        );
        assert_eq!(
            lib.track_artists(before.id).unwrap(),
            vec!["New Artist".to_string()]
        );
    }

    #[test]
    fn test_set_track_genres_skips_blank_names() {
        let (lib, _path) = create_test_db();
        let track_id = seed_track(&lib, "Song", "Album", "Artist");
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();

        lib.set_track_genres(
            track_id,
            &["".into(), "   ".into(), "Jazz".into(), "\t\n".into()],
        )
        .unwrap();

        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Jazz".to_string()],
            "a blank name must not become a genre row"
        );
    }

    #[test]
    fn test_set_track_genres_leaves_other_tracks_alone() {
        let (lib, _path) = create_test_db();
        let first = seed_track(&lib, "One", "Album", "Artist");
        let second = seed_track(&lib, "Two", "Album", "Artist");
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();

        lib.set_track_genres(first, &["Jazz".into()]).unwrap();
        lib.set_track_genres(second, &["Techno".into()]).unwrap();
        lib.set_track_genres(first, &["Rock".into()]).unwrap();

        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Rock".to_string(), "Techno".to_string()],
            "replacing one track's genres must not touch its siblings"
        );
    }

    #[test]
    fn test_set_track_genres_keeps_a_genre_another_track_still_uses() {
        let (lib, _path) = create_test_db();
        let first = seed_track(&lib, "One", "Album", "Artist");
        let second = seed_track(&lib, "Two", "Album", "Artist");
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();

        lib.set_track_genres(first, &["Ambient".into()]).unwrap();
        lib.set_track_genres(second, &["Ambient".into()]).unwrap();
        lib.set_track_genres(first, &["Techno".into()]).unwrap();

        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Ambient".to_string(), "Techno".to_string()],
            "the orphan sweep must only take genres nothing links to"
        );
    }

    #[test]
    fn test_set_track_genres_reuses_an_existing_row_for_a_different_casing() {
        let (lib, _path) = create_test_db();
        let first = seed_track(&lib, "One", "Album", "Artist");
        let second = seed_track(&lib, "Two", "Album", "Artist");
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();

        lib.set_track_genres(first, &["Ambient".into()]).unwrap();
        lib.set_track_genres(second, &["AMBIENT".into()]).unwrap();

        assert_eq!(
            sorted_album_genres(&lib, album_id),
            vec!["Ambient".to_string()],
            "genres resolve through the lowercase key, as a scan does"
        );
    }

    #[test]
    fn test_album_artists_survive_a_track_upsert() {
        let (lib, _path) = create_test_db();
        let credited = lib.upsert_artist("Album Artist").unwrap();
        let album_id = lib.upsert_album("Album", Some(2020), None).unwrap();
        lib.set_album_artists(album_id, &[(credited, 0)]).unwrap();

        let performer = lib.upsert_artist("Guest").unwrap();
        let track = NewTrack {
            path: "/music/one.flac".into(),
            title: Some("One".into()),
            album_title: Some("Album".into()),
            artist_names: vec!["Guest".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2020),
            duration_ms: Some(1000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, Some(album_id), &[(performer, 0)])
            .unwrap();

        assert_eq!(
            lib.album_artists(album_id).unwrap(),
            vec!["Album Artist".to_string()],
            "a per-track write must not rewrite the row its siblings share"
        );
    }

    fn insert_album_track(
        lib: &SqliteLibrary,
        path: &str,
        album_id: Option<i64>,
        track_number: Option<u32>,
        artist_ids: &[(i64, i32)],
    ) -> i64 {
        let track = NewTrack {
            path: path.into(),
            title: Some(path.into()),
            album_title: None,
            artist_names: Vec::new(),
            track_number,
            disc_number: Some(1),
            year: None,
            duration_ms: None,
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        lib.upsert_track(&track, album_id, artist_ids).unwrap()
    }

    fn count_of(artists: &[ArtistSummary], id: i64) -> Option<i64> {
        artists.iter().find(|a| a.id == id).map(|a| a.track_count)
    }

    #[test]
    fn album_artist_grouping_uses_the_tag_and_falls_back_to_the_track_artist() {
        let (lib, _path) = create_test_db();
        let band = lib.upsert_artist("Band").unwrap();
        let guest = lib.upsert_artist("Guest").unwrap();
        let album = lib.upsert_album("Split", Some(2020), None).unwrap();
        let tagged = insert_album_track(&lib, "/m/01.flac", Some(album), Some(1), &[(guest, 0)]);
        let untagged = insert_album_track(&lib, "/m/02.flac", Some(album), Some(2), &[(guest, 0)]);
        lib.set_track_album_artists(tagged, &[(band, 0)]).unwrap();

        let by_album = lib.artists(ArtistGrouping::AlbumArtist).unwrap();
        assert_eq!(count_of(&by_album, band), Some(1));
        assert_eq!(
            count_of(&by_album, guest),
            Some(2),
            "listed through the untagged track, the page then carries every credit"
        );

        let by_track = lib.artists(ArtistGrouping::TrackArtist).unwrap();
        assert_eq!(count_of(&by_track, band), None);
        assert_eq!(count_of(&by_track, guest), Some(2));

        let ids = |tracks: Vec<Track>| tracks.into_iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(
            ids(lib
                .tracks_by_artist(band, ArtistGrouping::AlbumArtist)
                .unwrap()),
            vec![tagged]
        );
        assert_eq!(
            ids(lib
                .tracks_by_artist(guest, ArtistGrouping::AlbumArtist)
                .unwrap()),
            vec![tagged, untagged]
        );
        assert_eq!(
            ids(lib
                .tracks_by_artist(guest, ArtistGrouping::TrackArtist)
                .unwrap()),
            vec![tagged, untagged]
        );
        assert_eq!(
            lib.track_album_artists(tagged).unwrap(),
            vec!["Band".to_string()]
        );
        assert!(lib.track_album_artists(untagged).unwrap().is_empty());
    }

    #[test]
    fn artist_summary_follows_the_grouping() {
        let (lib, _path) = create_test_db();
        let band = lib.upsert_artist("Band").unwrap();
        let guest = lib.upsert_artist("Guest").unwrap();
        let album = lib.upsert_album("Split", Some(2020), None).unwrap();
        let tagged = insert_album_track(&lib, "/m/01.flac", Some(album), Some(1), &[(guest, 0)]);
        lib.set_track_album_artists(tagged, &[(band, 0)]).unwrap();

        let summary = lib
            .artist_summary(band, ArtistGrouping::AlbumArtist)
            .unwrap()
            .unwrap();
        assert_eq!((summary.name.as_str(), summary.track_count), ("Band", 1));
        assert!(
            lib.artist_summary(band, ArtistGrouping::TrackArtist)
                .unwrap()
                .is_none()
        );
        assert!(
            lib.artist_summary(guest, ArtistGrouping::AlbumArtist)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            lib.artist_summary(guest, ArtistGrouping::TrackArtist)
                .unwrap()
                .map(|a| a.track_count),
            Some(1)
        );
        assert!(
            lib.artist_summary(NO_METADATA_ARTIST_ID, ArtistGrouping::AlbumArtist)
                .unwrap()
                .is_none()
        );

        insert_album_track(&lib, "/m/loose.flac", None, None, &[]);
        for grouping in [ArtistGrouping::TrackArtist, ArtistGrouping::AlbumArtist] {
            let orphan = lib
                .artist_summary(NO_METADATA_ARTIST_ID, grouping)
                .unwrap()
                .unwrap();
            assert_eq!((orphan.id, orphan.track_count), (NO_METADATA_ARTIST_ID, 1));
            assert_eq!(
                count_of(&lib.artists(grouping).unwrap(), NO_METADATA_ARTIST_ID),
                Some(1)
            );
        }
    }

    #[test]
    fn resolve_album_artists_ignores_insertion_order() {
        let (lib, _path) = create_test_db();
        let alpha = lib.upsert_artist("Alpha").unwrap();
        let zed = lib.upsert_artist("Zed").unwrap();
        let compiler = lib.upsert_artist("Compiler").unwrap();

        let comp = lib.upsert_album("Comp", Some(2021), None).unwrap();
        let second = insert_album_track(&lib, "/c/02.flac", Some(comp), Some(2), &[(zed, 0)]);
        lib.set_track_album_artists(second, &[(compiler, 0)])
            .unwrap();
        insert_album_track(&lib, "/c/01.flac", Some(comp), Some(1), &[(alpha, 0)]);
        lib.set_album_artists(comp, &[(zed, 0)]).unwrap();

        let plain = lib.upsert_album("Plain", Some(2022), None).unwrap();
        insert_album_track(&lib, "/p/b.flac", Some(plain), Some(2), &[(zed, 0)]);
        insert_album_track(&lib, "/p/a.flac", Some(plain), Some(1), &[(alpha, 0)]);

        lib.resolve_album_artists().unwrap();
        assert_eq!(
            lib.album_artists(comp).unwrap(),
            vec!["Compiler".to_string()]
        );
        assert_eq!(lib.album_artists(plain).unwrap(), vec!["Alpha".to_string()]);

        lib.resolve_album_artists().unwrap();
        assert_eq!(
            lib.album_artists(comp).unwrap(),
            vec!["Compiler".to_string()]
        );
        assert_eq!(lib.album_artists(plain).unwrap(), vec!["Alpha".to_string()]);
    }

    #[test]
    fn orphan_cleanup_keeps_an_artist_only_named_as_a_track_album_artist() {
        let (lib, _path) = create_test_db();
        let alpha = lib.upsert_artist("Alpha").unwrap();
        let only_tag = lib.upsert_artist("Only In Tag").unwrap();
        let album = lib.upsert_album("Comp", Some(2021), None).unwrap();
        insert_album_track(&lib, "/c/01.flac", Some(album), Some(1), &[(alpha, 0)]);
        let second = insert_album_track(&lib, "/c/02.flac", Some(album), Some(2), &[(alpha, 0)]);
        lib.set_track_album_artists(second, &[(only_tag, 0)])
            .unwrap();

        lib.resolve_album_artists().unwrap();
        lib.delete_orphaned_albums_and_artists().unwrap();

        assert_eq!(
            lib.album_artists(album).unwrap(),
            vec!["Only In Tag".to_string()]
        );
        assert_eq!(
            lib.track_album_artists(second).unwrap(),
            vec!["Only In Tag".to_string()]
        );
    }

    #[test]
    fn an_album_without_tags_is_credited_to_the_shared_primary_artist() {
        let (lib, _path) = create_test_db();
        let band = lib.upsert_artist("Band").unwrap();
        let featuring = lib.upsert_artist("Band Feat. Guest").unwrap();
        let album = lib.upsert_album("Record", Some(2020), None).unwrap();
        insert_album_track(&lib, "/r/01.flac", Some(album), Some(1), &[(band, 0)]);
        insert_album_track(&lib, "/r/02.flac", Some(album), Some(2), &[(featuring, 0)]);
        insert_album_track(&lib, "/r/03.flac", Some(album), Some(3), &[]);

        lib.resolve_album_artists().unwrap();
        assert_eq!(lib.album_artists(album).unwrap(), vec!["Band".to_string()]);
        assert!(lib.album_artist_known(album).unwrap());

        let by_album = lib.artists(ArtistGrouping::AlbumArtist).unwrap();
        assert_eq!(count_of(&by_album, band), Some(3));
        assert_eq!(count_of(&by_album, featuring), None);
        assert_eq!(count_of(&by_album, NO_METADATA_ARTIST_ID), None);
        assert_eq!(
            lib.tracks_by_artist(band, ArtistGrouping::AlbumArtist)
                .unwrap()
                .len(),
            3
        );

        let by_track = lib.artists(ArtistGrouping::TrackArtist).unwrap();
        assert_eq!(count_of(&by_track, band), Some(1));
        assert_eq!(count_of(&by_track, featuring), Some(1));
        assert_eq!(count_of(&by_track, NO_METADATA_ARTIST_ID), Some(1));
    }

    #[test]
    fn a_mixed_album_without_tags_keeps_each_track_under_its_own_artist() {
        let (lib, _path) = create_test_db();
        let alpha = lib.upsert_artist("Alpha").unwrap();
        let zed = lib.upsert_artist("Zed").unwrap();
        let album = lib.upsert_album("Comp", Some(2020), None).unwrap();
        insert_album_track(&lib, "/c/01.flac", Some(album), Some(1), &[(alpha, 0)]);
        insert_album_track(&lib, "/c/02.flac", Some(album), Some(2), &[(zed, 0)]);

        lib.resolve_album_artists().unwrap();
        assert_eq!(lib.album_artists(album).unwrap(), vec!["Alpha".to_string()]);
        assert!(!lib.album_artist_known(album).unwrap());

        let by_album = lib.artists(ArtistGrouping::AlbumArtist).unwrap();
        assert_eq!(count_of(&by_album, alpha), Some(1));
        assert_eq!(count_of(&by_album, zed), Some(1));
    }

    #[test]
    fn a_partly_tagged_album_pulls_its_untagged_tracks_under_the_tag() {
        let (lib, _path) = create_test_db();
        let band = lib.upsert_artist("Band").unwrap();
        let guest = lib.upsert_artist("Guest").unwrap();
        let album = lib.upsert_album("Split", Some(2020), None).unwrap();
        let tagged = insert_album_track(&lib, "/m/01.flac", Some(album), Some(1), &[(guest, 0)]);
        insert_album_track(&lib, "/m/02.flac", Some(album), Some(2), &[(guest, 0)]);
        lib.set_track_album_artists(tagged, &[(band, 0)]).unwrap();

        lib.resolve_album_artists().unwrap();
        assert!(lib.album_artist_known(album).unwrap());
        let by_album = lib.artists(ArtistGrouping::AlbumArtist).unwrap();
        assert_eq!(count_of(&by_album, band), Some(2));
        assert_eq!(count_of(&by_album, guest), None);
    }

    #[test]
    fn a_listed_artists_page_also_holds_the_tracks_they_are_only_credited_on() {
        let (lib, _path) = create_test_db();
        let mick = lib.upsert_artist("Mick Gordon").unwrap();
        let other = lib.upsert_artist("Other").unwrap();
        let various = lib.upsert_artist("Various Artists").unwrap();
        let solo = lib.upsert_album("Old Blood", Some(2015), None).unwrap();
        let comp = lib.upsert_album("New Colossus", Some(2017), None).unwrap();
        let solo_track = insert_album_track(&lib, "/s/01.flac", Some(solo), Some(1), &[(mick, 0)]);
        let credited = insert_album_track(&lib, "/c/01.flac", Some(comp), Some(1), &[(mick, 0)]);
        let others = insert_album_track(&lib, "/c/02.flac", Some(comp), Some(2), &[(other, 0)]);
        lib.set_track_album_artists(credited, &[(various, 0)])
            .unwrap();
        lib.set_track_album_artists(others, &[(various, 0)])
            .unwrap();
        lib.resolve_album_artists().unwrap();

        let listed = lib.artists(ArtistGrouping::AlbumArtist).unwrap();
        assert_eq!(count_of(&listed, various), Some(2));
        assert_eq!(
            count_of(&listed, mick),
            Some(2),
            "his own album plus the credit"
        );
        assert_eq!(
            count_of(&listed, other),
            None,
            "credits alone do not list an artist"
        );

        let ids = |tracks: Vec<Track>| tracks.into_iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(
            ids(lib
                .tracks_by_artist(mick, ArtistGrouping::AlbumArtist)
                .unwrap()),
            vec![solo_track, credited]
        );
        assert_eq!(
            lib.artist_summary(mick, ArtistGrouping::AlbumArtist)
                .unwrap()
                .map(|a| a.track_count),
            Some(2)
        );
        assert!(
            lib.artist_summary(other, ArtistGrouping::AlbumArtist)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            lib.artist_summary(other, ArtistGrouping::TrackArtist)
                .unwrap()
                .map(|a| a.track_count),
            Some(1)
        );

        let haystacks = lib
            .artist_search_haystacks(ArtistGrouping::AlbumArtist)
            .unwrap();
        let various_hay = &haystacks[&various];
        assert!(various_hay.starts_with("Various Artists"));
        assert!(various_hay.contains("Mick Gordon") && various_hay.contains("Other"));
        assert_eq!(haystacks[&mick].trim(), "Mick Gordon");
        assert!(
            !haystacks.contains_key(&other),
            "only listed artists are searchable"
        );
    }

    #[test]
    fn set_album_artists_marks_the_albums_artist_known() {
        let (lib, _path) = create_test_db();
        let band = lib.upsert_artist("Band").unwrap();
        let album = lib.upsert_album("Record", Some(2020), None).unwrap();
        assert!(!lib.album_artist_known(album).unwrap());

        lib.set_album_artists(album, &[(band, 0)]).unwrap();
        assert!(lib.album_artist_known(album).unwrap());

        lib.set_album_artists(album, &[]).unwrap();
        assert!(
            !lib.album_artist_known(album).unwrap(),
            "clearing the credits clears the flag, so the two can never disagree"
        );
    }

    #[test]
    fn avatar_covers_skip_artists_the_grouping_does_not_list() {
        let (lib, _path) = create_test_db();
        let cover = {
            let img = image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut buf, image::ImageFormat::Jpeg)
                .unwrap();
            lib.save_cover_art(&buf.into_inner(), "/c/01.flac", true)
                .unwrap()
        };
        let headliner = lib.upsert_artist("Headliner").unwrap();
        let guest = lib.upsert_artist("Guest Only").unwrap();
        let various = lib.upsert_artist("Various Artists").unwrap();
        let comp = lib.upsert_album("Comp", Some(2020), Some(cover)).unwrap();
        let own = lib.upsert_album("Solo", Some(2021), Some(cover)).unwrap();
        let on_comp = insert_album_track(&lib, "/c/01.flac", Some(comp), Some(1), &[(guest, 0)]);
        lib.set_track_album_artists(on_comp, &[(various, 0)])
            .unwrap();
        let solo = insert_album_track(&lib, "/s/01.flac", Some(own), Some(1), &[(headliner, 0)]);
        lib.set_track_album_artists(solo, &[(headliner, 0)])
            .unwrap();
        lib.resolve_album_artists().unwrap();

        let covers = lib
            .artist_album_covers(ArtistGrouping::AlbumArtist)
            .unwrap();
        assert!(covers.contains_key(&various));
        assert!(covers.contains_key(&headliner));
        assert!(
            !covers.contains_key(&guest),
            "an artist the tab does not list never has an avatar to draw"
        );

        let by_track = lib
            .artist_album_covers(ArtistGrouping::TrackArtist)
            .unwrap();
        assert!(
            by_track.contains_key(&guest),
            "the other mode does list them"
        );
    }

    #[test]
    fn track_artist_grouping_searches_names_only() {
        let (lib, _path) = create_test_db();
        let band = lib.upsert_artist("Band").unwrap();
        let guest = lib.upsert_artist("Guest").unwrap();
        let album = lib.upsert_album("Record", Some(2020), None).unwrap();
        let shared = insert_album_track(
            &lib,
            "/r/01.flac",
            Some(album),
            Some(1),
            &[(band, 0), (guest, 1)],
        );
        lib.set_track_album_artists(shared, &[(band, 0)]).unwrap();
        lib.resolve_album_artists().unwrap();

        let by_track = lib
            .artist_search_haystacks(ArtistGrouping::TrackArtist)
            .unwrap();
        assert_eq!(by_track[&band].trim(), "Band");
        assert_eq!(by_track[&guest].trim(), "Guest");

        let by_album = lib
            .artist_search_haystacks(ArtistGrouping::AlbumArtist)
            .unwrap();
        assert!(
            by_album[&band].contains("Guest"),
            "searching for a guest must surface the artist whose page holds them"
        );
        assert!(!by_album.contains_key(&guest));
    }

    fn a_play_at(qualified: bool, started_at: u64) -> models::NewPlay {
        models::NewPlay {
            started_at,
            ..a_play(qualified)
        }
    }

    fn a_play(qualified: bool) -> models::NewPlay {
        models::NewPlay {
            track_id: None,
            artist: "Tool".into(),
            title: "Pneuma".into(),
            album: Some("Fear Inoculum".into()),
            album_artist: Some("Tool".into()),
            track_number: Some(2),
            duration_secs: Some(713),
            played_secs: Some(400),
            started_at: 1_700_000_000,
            qualified,
        }
    }

    fn count_rows(db_path: &PathBuf, sql: &str) -> i64 {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn play_tallies_group_case_insensitively_and_honour_since() {
        let (lib, _path) = create_test_db();
        lib.record_play(&a_play_at(true, 100)).unwrap();
        lib.record_play(
            &models::NewPlay {
                artist: "TOOL".into(),
                title: "pneuma".into(),
                album: Some("fear inoculum".into()),
                ..a_play_at(false, 200)
            },
        )
        .unwrap();
        lib.record_play(
            &models::NewPlay {
                title: "Invincible".into(),
                ..a_play_at(true, 300)
            },
        )
        .unwrap();

        let all = lib.play_tallies(None).unwrap();
        assert_eq!(all.len(), 2);
        let pneuma = all
            .iter()
            .find(|t| t.title.eq_ignore_ascii_case("pneuma"))
            .unwrap();
        assert_eq!(pneuma.plays, 2);
        assert_eq!(pneuma.qualified_plays, 1);
        assert_eq!(pneuma.last_played, 200);

        let recent = lib.play_tallies(Some(250)).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].title, "Invincible");
    }

    #[test]
    fn recent_plays_are_newest_first_and_limited() {
        let (lib, _path) = create_test_db();
        for at in [10, 30, 20] {
            lib.record_play(&a_play_at(true, at)).unwrap();
        }
        let recent = lib.recent_plays(2).unwrap();
        let times: Vec<u64> = recent.iter().map(|p| p.started_at).collect();
        assert_eq!(times, vec![30, 20]);
    }

    #[test]
    fn track_listings_carry_names_and_play_stats() {
        let (lib, _path) = create_test_db();
        let artist_id = lib.upsert_artist("Tool").unwrap();
        let album_id = lib.upsert_album("Fear Inoculum", Some(2019), None).unwrap();
        lib.set_album_artists(album_id, &[(artist_id, 0)]).unwrap();
        let new_track = |path: &str, title: &str| NewTrack {
            path: path.into(),
            title: Some(title.into()),
            album_title: Some("Fear Inoculum".into()),
            artist_names: vec!["Tool".into()],
            track_number: Some(1),
            disc_number: Some(1),
            year: None,
            duration_ms: Some(700_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let played = lib
            .upsert_track(
                &new_track("/m/pneuma.flac", "Pneuma"),
                Some(album_id),
                &[(artist_id, 0)],
            )
            .unwrap();
        let unplayed = lib
            .upsert_track(
                &new_track("/m/descending.flac", "Descending"),
                Some(album_id),
                &[(artist_id, 0)],
            )
            .unwrap();
        for at in [100, 500] {
            lib.record_play(
                &models::NewPlay {
                    track_id: Some(played),
                    ..a_play_at(true, at)
                },
            )
            .unwrap();
        }

        let listings = lib.track_listings().unwrap();
        let by_id = |id: i64| listings.iter().find(|l| l.track_id == id).unwrap();
        let p = by_id(played);
        assert_eq!(p.artist, "Tool");
        assert_eq!(p.album_id, Some(album_id));
        assert_eq!(p.album.as_deref(), Some("Fear Inoculum"));
        assert_eq!(p.album_artist.as_deref(), Some("Tool"));
        assert_eq!(p.year, Some(2019));
        assert_eq!((p.plays, p.last_played), (2, Some(500)));
        let u = by_id(unplayed);
        assert_eq!((u.plays, u.last_played), (0, None));
    }

    #[test]
    fn a_play_is_recorded() {
        let (lib, path) = create_test_db();
        lib.record_play(&a_play(true)).unwrap();
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM plays"), 1);
    }

    #[test]
    fn an_unqualified_play_is_history_only() {
        let (lib, path) = create_test_db();
        lib.record_play(&a_play(false)).unwrap();
        assert_eq!(
            count_rows(&path, "SELECT COUNT(*) FROM plays WHERE qualified = 0"),
            1
        );
    }

    #[test]
    fn history_survives_a_track_disappearing_from_the_library() {
        let (lib, path) = create_test_db();
        let artist = lib.upsert_artist("Tool").unwrap();
        let new_track = NewTrack {
            path: "/m/pneuma.flac".into(),
            title: Some("Pneuma".into()),
            album_title: None,
            artist_names: vec!["Tool".into()],
            track_number: Some(2),
            disc_number: Some(1),
            year: None,
            duration_ms: Some(713_000),
            cover_art_id: None,
            start_offset_ms: None,
            bitrate: None,
        };
        let track = lib.upsert_track(&new_track, None, &[(artist, 0)]).unwrap();
        let mut play = a_play(true);
        play.track_id = Some(track);
        lib.record_play(&play).unwrap();

        lib.clear().unwrap();

        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM plays"), 1);
        assert_eq!(
            count_rows(
                &path,
                &format!("SELECT COUNT(*) FROM plays WHERE track_id = {track} AND artist = 'Tool'")
            ),
            1
        );
    }

    #[test]
    fn recording_the_same_listen_twice_merges_and_never_shrinks_it() {
        let (lib, path) = create_test_db();
        let mut early = a_play(true);
        early.played_secs = Some(200);
        let first = lib.record_play(&early).unwrap();

        let mut total = a_play(true);
        total.played_secs = Some(500);
        let second = lib.record_play(&total).unwrap();

        assert_eq!(first, second, "one listen is one row");
        assert_eq!(count_rows(&path, "SELECT COUNT(*) FROM plays"), 1);
        assert_eq!(count_rows(&path, "SELECT played_secs FROM plays"), 500);

        let mut shorter = a_play(true);
        shorter.played_secs = Some(120);
        lib.record_play(&shorter).unwrap();
        assert_eq!(
            count_rows(&path, "SELECT played_secs FROM plays"),
            500,
            "a late commit must never shrink what was already recorded"
        );
    }

    #[test]
    fn an_unqualified_listen_can_be_upgraded_when_it_later_qualifies() {
        let (lib, path) = create_test_db();
        lib.record_play(&a_play(false)).unwrap();
        assert_eq!(count_rows(&path, "SELECT qualified FROM plays"), 0);

        lib.record_play(&a_play(true)).unwrap();

        assert_eq!(
            count_rows(&path, "SELECT qualified FROM plays"),
            1,
            "a later qualified commit must upgrade the row, never the reverse"
        );
    }

    #[test]
    fn a_restored_backup_replaces_the_library_and_keeps_what_it_replaced() {
        let (lib, path) = create_test_db();
        seed_track(&lib, "Before", "Album", "Artist");
        let backup = lib
            .daily_backup(backup::DEFAULT_KEEP_DAILY_BACKUPS)
            .unwrap()
            .expect("a backup is taken once a day");
        let backup_path = std::path::PathBuf::from(format!(
            "{}.bak-for-test",
            path.display()
        ));
        std::fs::copy(&backup, &backup_path).unwrap();

        // Move the library on from the backed-up state.
        seed_track(&lib, "After", "Album", "Artist");
        assert_eq!(lib.track_count().unwrap(), 2);

        lib.request_restore(&backup_path).unwrap();
        drop(lib);

        let restored = SqliteLibrary::open_at(&path).unwrap();
        assert_eq!(
            restored.track_count().unwrap(),
            1,
            "the backup's contents must win"
        );
        let titles: Vec<String> = restored
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, vec!["Before".to_string()]);

        // The restore is itself undoable: what it replaced is still on disk.
        let prefix = format!(
            "{}.replaced-",
            path.file_name().unwrap().to_string_lossy()
        );
        let aside: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix))
            .collect();
        assert_eq!(aside.len(), 1, "the replaced database must be kept: {aside:?}");
    }

    #[test]
    fn a_restore_marker_that_points_at_nothing_is_ignored() {
        let (lib, path) = create_test_db();
        seed_track(&lib, "Kept", "Album", "Artist");
        lib.request_restore(std::path::Path::new("/nonexistent/backup.db"))
            .unwrap();
        drop(lib);

        let reopened = SqliteLibrary::open_at(&path).unwrap();
        assert_eq!(
            reopened.track_count().unwrap(),
            1,
            "a dead marker must not cost the user their library"
        );
        assert!(
            !std::path::PathBuf::from(format!("{}.restore-pending", path.display())).exists(),
            "and it must not survive to try again next launch"
        );
    }

    #[test]
    fn the_integrity_report_is_clean_on_a_healthy_library() {
        let (lib, _path) = create_test_db();
        seed_track(&lib, "Song", "Album", "Artist");

        let report = lib.integrity_report().unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.integrity, "ok");
    }

    #[test]
    fn the_integrity_report_counts_a_playlist_entry_whose_track_is_gone() {
        let (lib, path) = create_test_db();
        let track = seed_track(&lib, "Song", "Album", "Artist");
        let playlist = lib.create_playlist("Kept").unwrap();
        lib.add_track_to_playlist(playlist, track).unwrap();

        // Reach around the library's own constraints: a dangling row can only
        // arrive from a bad migration, which is exactly what this is looking for.
        {
            let raw = rusqlite::Connection::open(&path).unwrap();
            raw.pragma_update(None, "foreign_keys", "OFF").unwrap();
            raw.execute(
                "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?1, 99, 999999)",
                [playlist],
            )
            .unwrap();
        }

        let report = lib.integrity_report().unwrap();
        assert_eq!(report.dangling_playlist_entries, 1, "{report:?}");
        assert!(!report.is_clean());
    }

    /// A rescan deletes every track, so a history table with no index on its
    /// foreign key makes SQLite full-scan it once per deleted row. `plays` is the
    /// only history table left — `loves` went with the Liked feature in 4.0.
    #[test]
    fn the_remaining_history_table_has_an_index_for_its_foreign_key() {
        let (_lib, path) = create_test_db();
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name = ?1")
            .unwrap();
        assert!(
            stmt.exists(["idx_plays_track"]).unwrap(),
            "idx_plays_track is missing: a rescan deletes every track, and without it SQLite \
             full-scans the history table once per deleted row"
        );
        drop(stmt);
        assert_eq!(
            count_rows(
                &path,
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'loves'"
            ),
            0
        );
    }

    fn genre_track(
        path: &str,
        album: Option<(&str, &str, Option<i32>)>,
        track_number: u32,
        genres: &[&str],
    ) -> ScanTrack {
        let (album_title, artist, year) = match album {
            Some((title, artist, year)) => (Some(title.to_string()), artist, year),
            None => (None, "Loner", None),
        };
        ScanTrack {
            path: path.into(),
            title: Some(path.into()),
            file_size: Some(content_size(path)),
            album_title,
            artist_names: vec![artist.into()],
            album_artist_names: vec![artist.into()],
            track_number: Some(track_number),
            disc_number: Some(1),
            year,
            genres: genres.iter().map(|g| g.to_string()).collect(),
            duration_ms: Some(180_000),
            ..Default::default()
        }
    }

    fn genre_titles(lib: &SqliteLibrary, key: &str, sort: GenreSort) -> Vec<String> {
        lib.tracks_by_genre(key, sort)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    }

    #[test]
    fn genres_merge_casing_and_count_each_track_once() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                genre_track("/m/a.flac", Some(("A", "Band", Some(2000))), 1, &["Rock"]),
                genre_track(
                    "/m/b.flac",
                    Some(("A", "Band", Some(2000))),
                    2,
                    &["rock", "Jazz"],
                ),
                genre_track("/m/c.flac", Some(("A", "Band", Some(2000))), 3, &[]),
                genre_track("/m/d.flac", Some(("A", "Band", Some(2000))), 4, &["Jazz"]),
            ],
        );

        let genres = lib.genres().unwrap();
        let listed: Vec<(&str, i64)> = genres
            .iter()
            .map(|g| (g.key.as_str(), g.track_count))
            .collect();
        assert_eq!(listed, vec![("jazz", 2), ("rock", 2)]);
        assert_eq!(genres[1].name, "Rock", "shown under the first spelling met");
    }

    #[test]
    fn a_genre_whose_tracks_are_gone_is_not_listed() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                genre_track("/m/a.flac", Some(("A", "Band", Some(2000))), 1, &["Rock"]),
                genre_track("/m/b.flac", Some(("A", "Band", Some(2000))), 2, &["Jazz"]),
            ],
        );
        scan(
            &lib,
            vec![genre_track(
                "/m/a.flac",
                Some(("A", "Band", Some(2000))),
                1,
                &["Rock"],
            )],
        );

        let keys: Vec<String> = lib.genres().unwrap().into_iter().map(|g| g.key).collect();
        assert_eq!(keys, vec!["rock".to_string()]);
        assert!(
            lib.tracks_by_genre("jazz", GenreSort::Artist)
                .unwrap()
                .is_empty()
        );
    }

    #[rstest::rstest]
    #[case::by_artist(
        GenreSort::Artist,
        &["/m/aardvark.flac", "/m/alpha.flac", "/m/beta-1990.flac", "/m/beta-2010-1.flac",
          "/m/beta-2010-2.flac", "/m/loner.flac"]
    )]
    #[case::by_year(
        GenreSort::Year,
        &["/m/beta-1990.flac", "/m/alpha.flac", "/m/beta-2010-1.flac", "/m/beta-2010-2.flac",
          "/m/aardvark.flac", "/m/loner.flac"]
    )]
    fn tracks_by_genre_keep_albums_whole_in_the_chosen_order(
        #[case] sort: GenreSort,
        #[case] expected: &[&str],
    ) {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                genre_track(
                    "/m/beta-2010-2.flac",
                    Some(("Late", "Beta", Some(2010))),
                    2,
                    &["Rock"],
                ),
                genre_track(
                    "/m/beta-2010-1.flac",
                    Some(("Late", "Beta", Some(2010))),
                    1,
                    &["Rock"],
                ),
                genre_track("/m/loner.flac", None, 1, &["Rock"]),
                genre_track(
                    "/m/beta-1990.flac",
                    Some(("Early", "Beta", Some(1990))),
                    1,
                    &["Rock"],
                ),
                genre_track(
                    "/m/aardvark.flac",
                    Some(("Undated", "Aardvark", None)),
                    1,
                    &["Rock"],
                ),
                genre_track(
                    "/m/alpha.flac",
                    Some(("Mid", "Alpha", Some(2000))),
                    1,
                    &["Rock"],
                ),
                genre_track(
                    "/m/alpha-jazz.flac",
                    Some(("Mid", "Alpha", Some(2000))),
                    2,
                    &["Jazz"],
                ),
            ],
        );
        lib.resolve_album_artists().unwrap();

        assert_eq!(genre_titles(&lib, "rock", sort), expected);
    }

    #[test]
    fn tracks_by_genre_resolve_by_the_lowercased_key() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                genre_track("/m/a.flac", Some(("A", "Band", Some(2000))), 1, &["ROCK"]),
                genre_track("/m/b.flac", Some(("A", "Band", Some(2000))), 2, &["Rock"]),
            ],
        );

        assert_eq!(
            genre_titles(&lib, "rock", GenreSort::Artist),
            vec!["/m/a.flac".to_string(), "/m/b.flac".to_string()]
        );
        assert!(genre_titles(&lib, "Rock", GenreSort::Artist).is_empty());
        assert!(genre_titles(&lib, "polka", GenreSort::Artist).is_empty());
    }

    #[test]
    fn genre_covers_are_capped_at_three_most_used_album_first() {
        let (lib, _path) = create_test_db();
        let cover = |rgb: [u8; 3]| {
            lib.save_cover_art(&make_test_jpeg(&rgb), "/music/a.flac", true)
                .unwrap()
        };
        let (c1, c2, c3, c4) = (
            cover([255, 0, 0]),
            cover([0, 255, 0]),
            cover([0, 0, 255]),
            cover([128, 128, 0]),
        );
        let artist = lib.upsert_artist("Band").unwrap();
        let mut n = 0;
        let mut add = |title: &str, cover: Option<i64>, tracks: usize, genre: &str| {
            let album = lib.upsert_album(title, Some(2000), cover).unwrap();
            for _ in 0..tracks {
                n += 1;
                let path = format!("/m/{n}.flac");
                let id = insert_album_track(&lib, &path, Some(album), Some(n), &[(artist, 0)]);
                lib.set_track_genres(id, &[genre.to_string()]).unwrap();
            }
        };
        add("B", Some(c2), 1, "Rock");
        add("A", Some(c1), 3, "Rock");
        add("D", Some(c4), 1, "Rock");
        add("C", Some(c3), 2, "Rock");
        add("Bare", None, 2, "Jazz");

        let covers = lib.genre_album_covers().unwrap();
        assert_eq!(covers.get("rock"), Some(&vec![c1, c3, c2]));
        assert!(!covers.contains_key("jazz"));
    }

    #[test]
    fn a_genre_on_a_track_without_an_album_leaves_album_genres_intact() {
        let (lib, _path) = create_test_db();
        scan(
            &lib,
            vec![
                genre_track("/m/a.flac", Some(("A", "Band", Some(2000))), 1, &["Rock"]),
                genre_track("/m/loner.flac", None, 1, &["Jazz"]),
            ],
        );

        let map = lib.album_genres_map().unwrap();
        let album = lib.albums().unwrap().into_iter().find(|a| a.title == "A");
        assert_eq!(map.get(&album.unwrap().id), Some(&vec!["Rock".to_string()]));
    }

    #[test]
    fn a_featured_credit_leads_to_its_listed_artist_but_a_compilation_guest_does_not() {
        let (lib, _path) = create_test_db();
        let track = |path: &str, title: &str, artist: &str, album_artist: &str, album: &str| {
            let mut t = scan_track(path, title);
            t.artist_names = vec![artist.into()];
            t.album_artist_names = vec![album_artist.into()];
            t.album_title = Some(album.into());
            t
        };
        scan(
            &lib,
            vec![
                track(
                    "/m/chippin.flac",
                    "Chippin' in",
                    "Samurai feat. Refused",
                    "Samurai",
                    "Chippin' in",
                ),
                track(
                    "/m/archangel.flac",
                    "Archangel",
                    "Samurai",
                    "Samurai",
                    "Archangel",
                ),
                track(
                    "/m/ost1.flac",
                    "Theme",
                    "Martin Stig Andersen",
                    "Various Artists",
                    "OST",
                ),
                track(
                    "/m/ost2.flac",
                    "Fight",
                    "Mick Gordon & Chad Mossholder",
                    "Mick Gordon",
                    "Doom",
                ),
            ],
        );
        lib.resolve_album_artists().unwrap();

        let id_of = |name: &str| {
            lib.artists(ArtistGrouping::TrackArtist)
                .unwrap()
                .into_iter()
                .chain(lib.artists(ArtistGrouping::AlbumArtist).unwrap())
                .find(|a| a.name == name)
                .unwrap()
                .id
        };
        let track_id = |title: &str| id_of_title(&lib, title);
        let listed = |title: &str, credit: &str, grouping| {
            lib.listed_artist_for_credit(track_id(title), id_of(credit), grouping)
                .unwrap()
        };

        assert_eq!(
            listed(
                "Chippin' in",
                "Samurai feat. Refused",
                ArtistGrouping::AlbumArtist
            ),
            Some(id_of("Samurai"))
        );
        assert_eq!(
            listed(
                "Fight",
                "Mick Gordon & Chad Mossholder",
                ArtistGrouping::AlbumArtist
            ),
            Some(id_of("Mick Gordon"))
        );
        assert_eq!(
            listed("Theme", "Martin Stig Andersen", ArtistGrouping::AlbumArtist),
            None,
            "a compilation guest keeps their own page"
        );
        assert_eq!(
            listed(
                "Chippin' in",
                "Samurai feat. Refused",
                ArtistGrouping::TrackArtist
            ),
            Some(id_of("Samurai feat. Refused"))
        );
    }
}
