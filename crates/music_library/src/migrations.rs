pub const MIGRATIONS: &[(i32, &str)] = &[
    (
        1,
        r#"
        CREATE TABLE artists (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            sort_name TEXT NOT NULL
        );

        CREATE UNIQUE INDEX idx_artists_name ON artists(name);
        CREATE INDEX idx_artists_sort_name ON artists(sort_name);

        CREATE TABLE cover_art (
            id INTEGER PRIMARY KEY,
            hash TEXT NOT NULL,
            small BLOB NOT NULL,
            large BLOB NOT NULL
        );

        CREATE UNIQUE INDEX idx_cover_art_hash ON cover_art(hash);

        CREATE TABLE albums (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            year INTEGER,
            cover_art_id INTEGER REFERENCES cover_art(id) ON DELETE SET NULL
        );

        CREATE INDEX idx_albums_title ON albums(title);
        CREATE INDEX idx_albums_year ON albums(year);

        CREATE TABLE album_artists (
            album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
            artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (album_id, artist_id)
        );

        CREATE TABLE tracks (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            album_id INTEGER REFERENCES albums(id) ON DELETE SET NULL,
            track_number INTEGER,
            disc_number INTEGER NOT NULL DEFAULT 1,
            duration_ms INTEGER,
            year INTEGER,
            cover_art_id INTEGER REFERENCES cover_art(id) ON DELETE SET NULL,
            start_offset_ms INTEGER NOT NULL DEFAULT 0,
            liked INTEGER NOT NULL DEFAULT 0,
            bitrate INTEGER
        );

        CREATE INDEX idx_tracks_liked ON tracks(liked) WHERE liked = 1;

        CREATE UNIQUE INDEX idx_tracks_path_offset ON tracks(path, start_offset_ms);

        CREATE INDEX idx_tracks_album_sort ON tracks(album_id, disc_number, track_number);

        CREATE TABLE track_artists (
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
            role TEXT NOT NULL DEFAULT 'main',
            credited_as TEXT,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, artist_id, role, position)
        );

        CREATE INDEX idx_track_artists_artist_id ON track_artists(artist_id, track_id);

        CREATE TABLE playlists (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX idx_playlists_created_at ON playlists(created_at);

        CREATE TABLE playlist_tracks (
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            position INTEGER NOT NULL,
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            PRIMARY KEY (playlist_id, position)
        );

        CREATE INDEX idx_playlist_tracks_track_id ON playlist_tracks(track_id);
        CREATE UNIQUE INDEX idx_playlist_tracks_pair
            ON playlist_tracks(playlist_id, track_id);

        CREATE TABLE scan_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    ),
    (
        2,
        r#"
        ALTER TABLE cover_art ADD COLUMN source_path TEXT;
        ALTER TABLE cover_art ADD COLUMN embedded INTEGER NOT NULL DEFAULT 0;
        "#,
    ),
    (
        3,
        r#"
        CREATE TABLE genres (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            key TEXT NOT NULL
        );

        CREATE UNIQUE INDEX idx_genres_key ON genres(key);

        CREATE TABLE track_genres (
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            genre_id INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, genre_id)
        );

        CREATE INDEX idx_track_genres_genre ON track_genres(genre_id, track_id);
        "#,
    ),
    (
        4,
        r#"
        CREATE TABLE lyrics (
            track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
            source     TEXT    NOT NULL,
            text       BLOB    NOT NULL,
            not_found  INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL
        );
        "#,
    ),
    (
        5,
        r#"
        ALTER TABLE tracks ADD COLUMN is_cue INTEGER NOT NULL DEFAULT 0;
        "#,
    ),
    (
        6,
        r#"
        CREATE TABLE track_album_artists (
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (track_id, artist_id)
        );
        CREATE INDEX idx_track_album_artists_artist_id ON track_album_artists(artist_id, track_id);
        "#,
    ),
    (
        7,
        r#"
        ALTER TABLE albums ADD COLUMN artist_known INTEGER NOT NULL DEFAULT 0;
        "#,
    ),
    (
        8,
        r#"
        CREATE TABLE plays (
            id INTEGER PRIMARY KEY,
            track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
            artist TEXT NOT NULL,
            title TEXT NOT NULL,
            album TEXT,
            album_artist TEXT,
            track_number INTEGER,
            duration_secs INTEGER,
            played_secs INTEGER,
            started_at INTEGER NOT NULL,
            qualified INTEGER NOT NULL DEFAULT 0
        );
        CREATE UNIQUE INDEX idx_plays_identity ON plays(started_at, artist, title);
        CREATE INDEX idx_plays_started_at ON plays(started_at);
        CREATE INDEX idx_plays_track ON plays(track_id) WHERE track_id IS NOT NULL;

        CREATE TABLE loves (
            id INTEGER PRIMARY KEY,
            track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
            artist TEXT NOT NULL,
            title TEXT NOT NULL,
            loved INTEGER NOT NULL,
            at INTEGER NOT NULL
        );
        CREATE INDEX idx_loves_at ON loves(at);
        CREATE INDEX idx_loves_track ON loves(track_id) WHERE track_id IS NOT NULL;

        CREATE TABLE play_deliveries (
            play_id INTEGER NOT NULL REFERENCES plays(id) ON DELETE CASCADE,
            target TEXT NOT NULL,
            state INTEGER NOT NULL DEFAULT 0,
            attempts INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (play_id, target)
        );
        CREATE INDEX idx_play_deliveries_pending ON play_deliveries(target, play_id) WHERE state = 0;

        CREATE TABLE love_deliveries (
            love_id INTEGER NOT NULL REFERENCES loves(id) ON DELETE CASCADE,
            target TEXT NOT NULL,
            state INTEGER NOT NULL DEFAULT 0,
            attempts INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (love_id, target)
        );
        CREATE INDEX idx_love_deliveries_pending ON love_deliveries(target, love_id) WHERE state = 0;
        "#,
    ),
    (
        9,
        r#"
        CREATE TABLE sources (
            id INTEGER PRIMARY KEY,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            uri TEXT NOT NULL,
            config TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            available INTEGER NOT NULL DEFAULT 1,
            cursor TEXT,
            last_sync_at INTEGER,
            last_sync_ok INTEGER,
            last_error TEXT
        );
        CREATE UNIQUE INDEX idx_sources_kind_uri ON sources(kind, uri);
        INSERT INTO sources (id, kind, name, uri, enabled) VALUES (1, 'local', '', '', 0);

        CREATE TABLE media_items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL DEFAULT 'track',
            title TEXT NOT NULL DEFAULT '',
            artist TEXT NOT NULL DEFAULT '',
            album TEXT,
            duration_ms INTEGER,
            cover_art_id INTEGER REFERENCES cover_art(id) ON DELETE SET NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE media_bindings (
            id INTEGER PRIMARY KEY,
            item_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            source_key TEXT NOT NULL,
            start_offset_ms INTEGER NOT NULL DEFAULT 0,
            present INTEGER NOT NULL DEFAULT 1,
            last_seen_scan INTEGER NOT NULL DEFAULT 0,
            last_seen_at INTEGER,
            file_size INTEGER
        );
        CREATE UNIQUE INDEX idx_media_bindings_key
            ON media_bindings(source_id, source_key, start_offset_ms);
        CREATE INDEX idx_media_bindings_item ON media_bindings(item_id);
        CREATE INDEX idx_media_bindings_path ON media_bindings(source_key, start_offset_ms);

        INSERT INTO media_items
            (id, kind, title, artist, album, duration_ms, cover_art_id, created_at, updated_at)
        SELECT
            t.id,
            'track',
            t.title,
            COALESCE((
                SELECT a.name FROM track_artists ta
                JOIN artists a ON a.id = ta.artist_id
                WHERE ta.track_id = t.id
                ORDER BY ta.position
                LIMIT 1
            ), ''),
            (SELECT al.title FROM albums al WHERE al.id = t.album_id),
            t.duration_ms,
            t.cover_art_id,
            CAST(strftime('%s', 'now') AS INTEGER),
            CAST(strftime('%s', 'now') AS INTEGER)
        FROM tracks t;

        INSERT INTO media_bindings (item_id, source_id, source_key, start_offset_ms, present)
        SELECT id, 1, path, start_offset_ms, 1 FROM tracks;

        CREATE TABLE tracks_new (
            id INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            album_id INTEGER REFERENCES albums(id) ON DELETE SET NULL,
            track_number INTEGER,
            disc_number INTEGER NOT NULL DEFAULT 1,
            duration_ms INTEGER,
            year INTEGER,
            cover_art_id INTEGER REFERENCES cover_art(id) ON DELETE SET NULL,
            start_offset_ms INTEGER NOT NULL DEFAULT 0,
            bitrate INTEGER,
            is_cue INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO tracks_new
            (id, path, title, album_id, track_number, disc_number, duration_ms, year,
             cover_art_id, start_offset_ms, bitrate, is_cue)
        SELECT
            id, path, title, album_id, track_number, disc_number, duration_ms, year,
            cover_art_id, start_offset_ms, bitrate, is_cue
        FROM tracks;

        CREATE TABLE playlist_tracks_new (
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            position INTEGER NOT NULL,
            track_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            PRIMARY KEY (playlist_id, position)
        );
        INSERT INTO playlist_tracks_new (playlist_id, position, track_id)
        SELECT playlist_id, position, track_id FROM playlist_tracks
        WHERE track_id IN (SELECT id FROM media_items)
          AND playlist_id IN (SELECT id FROM playlists);

        CREATE TABLE lyrics_new (
            track_id   INTEGER PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE,
            source     TEXT    NOT NULL,
            text       BLOB    NOT NULL,
            not_found  INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL
        );
        INSERT INTO lyrics_new (track_id, source, text, not_found, updated_at)
        SELECT track_id, source, text, not_found, updated_at FROM lyrics
        WHERE track_id IN (SELECT id FROM media_items);

        CREATE TABLE plays_new (
            id INTEGER PRIMARY KEY,
            track_id INTEGER REFERENCES media_items(id) ON DELETE SET NULL,
            artist TEXT NOT NULL,
            title TEXT NOT NULL,
            album TEXT,
            album_artist TEXT,
            track_number INTEGER,
            duration_secs INTEGER,
            played_secs INTEGER,
            started_at INTEGER NOT NULL,
            qualified INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO plays_new
            (id, track_id, artist, title, album, album_artist, track_number,
             duration_secs, played_secs, started_at, qualified)
        SELECT
            id,
            CASE WHEN track_id IN (SELECT id FROM media_items) THEN track_id END,
            artist, title, album, album_artist, track_number,
            duration_secs, played_secs, started_at, qualified
        FROM plays;

        CREATE TABLE loves_new (
            id INTEGER PRIMARY KEY,
            track_id INTEGER REFERENCES media_items(id) ON DELETE SET NULL,
            artist TEXT NOT NULL,
            title TEXT NOT NULL,
            loved INTEGER NOT NULL,
            at INTEGER NOT NULL
        );
        INSERT INTO loves_new (id, track_id, artist, title, loved, at)
        SELECT
            id,
            CASE WHEN track_id IN (SELECT id FROM media_items) THEN track_id END,
            artist, title, loved, at
        FROM loves;

        DROP TABLE playlist_tracks;
        DROP TABLE lyrics;
        DROP TABLE plays;
        DROP TABLE loves;
        DROP TABLE tracks;

        ALTER TABLE tracks_new RENAME TO tracks;
        ALTER TABLE playlist_tracks_new RENAME TO playlist_tracks;
        ALTER TABLE lyrics_new RENAME TO lyrics;
        ALTER TABLE plays_new RENAME TO plays;
        ALTER TABLE loves_new RENAME TO loves;

        CREATE UNIQUE INDEX idx_tracks_path_offset ON tracks(path, start_offset_ms);
        CREATE INDEX idx_tracks_album_sort ON tracks(album_id, disc_number, track_number);
        CREATE INDEX idx_playlist_tracks_track_id ON playlist_tracks(track_id);
        CREATE UNIQUE INDEX idx_playlist_tracks_pair ON playlist_tracks(playlist_id, track_id);
        CREATE UNIQUE INDEX idx_plays_identity ON plays(started_at, artist, title);
        CREATE INDEX idx_plays_started_at ON plays(started_at);
        CREATE INDEX idx_plays_track ON plays(track_id) WHERE track_id IS NOT NULL;
        CREATE INDEX idx_loves_at ON loves(at);
        CREATE INDEX idx_loves_track ON loves(track_id) WHERE track_id IS NOT NULL;

        CREATE VIEW liked_track_ids AS
        SELECT track_id FROM playlist_tracks
        WHERE playlist_id = (
            SELECT CAST(value AS INTEGER) FROM scan_meta WHERE key = 'liked_playlist_id'
        );

        CREATE TRIGGER media_items_guard_user_data BEFORE DELETE ON media_items
        WHEN EXISTS (SELECT 1 FROM playlist_tracks WHERE track_id = OLD.id)
          OR EXISTS (
              SELECT 1 FROM lyrics
              WHERE track_id = OLD.id AND source NOT IN ('lrc', 'embedded')
          )
          OR EXISTS (SELECT 1 FROM plays WHERE track_id = OLD.id)
          OR EXISTS (SELECT 1 FROM loves WHERE track_id = OLD.id)
        BEGIN
            SELECT RAISE(ABORT, 'media item is referenced by user data');
        END;

        CREATE TABLE adoptions (
            id INTEGER PRIMARY KEY,
            item_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            binding_id INTEGER NOT NULL REFERENCES media_bindings(id) ON DELETE CASCADE,
            tier TEXT NOT NULL,
            at INTEGER NOT NULL
        );
        CREATE INDEX idx_adoptions_item ON adoptions(item_id);
        CREATE INDEX idx_adoptions_binding ON adoptions(binding_id);

        CREATE TABLE remote_tracks (
            binding_id INTEGER PRIMARY KEY REFERENCES media_bindings(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            artist TEXT,
            album TEXT,
            album_artist TEXT,
            track_number INTEGER,
            disc_number INTEGER,
            year INTEGER,
            genre TEXT,
            duration_ms INTEGER,
            size INTEGER,
            suffix TEXT,
            content_type TEXT,
            bitrate INTEGER,
            cover_key TEXT,
            cover_hash TEXT,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX idx_remote_tracks_cover_hash ON remote_tracks(cover_hash);
        "#,
    ),
    (
        10,
        r#"
        DROP TRIGGER media_items_guard_user_data;

        CREATE TABLE lyrics_new (
            track_id   INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            source     TEXT    NOT NULL,
            text       BLOB    NOT NULL,
            not_found  INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (track_id, source)
        );
        INSERT INTO lyrics_new (track_id, source, text, not_found, updated_at)
        SELECT track_id, source, text, not_found, updated_at FROM lyrics;
        DROP TABLE lyrics;
        ALTER TABLE lyrics_new RENAME TO lyrics;

        CREATE TRIGGER media_items_guard_user_data BEFORE DELETE ON media_items
        WHEN EXISTS (SELECT 1 FROM playlist_tracks WHERE track_id = OLD.id)
          OR EXISTS (
              SELECT 1 FROM lyrics
              WHERE track_id = OLD.id AND source NOT IN ('lrc', 'embedded')
          )
          OR EXISTS (SELECT 1 FROM plays WHERE track_id = OLD.id)
          OR EXISTS (SELECT 1 FROM loves WHERE track_id = OLD.id)
        BEGIN
            SELECT RAISE(ABORT, 'media item is referenced by user data');
        END;
        "#,
    ),
    (
        11,
        r#"
        DELETE FROM scan_meta WHERE key = 'fingerprint'
        AND EXISTS (
            SELECT 1 FROM remote_tracks
            WHERE genre LIKE '%;%' OR genre LIKE '%,%' OR genre LIKE '%/%'
            OR genre <> trim(genre) OR genre LIKE '%  %'
            OR genre GLOB '*[' || char(9, 10, 11, 12, 13, 133, 160, 5760, 8192, 8193, 8194,
                8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287,
                12288) || ']*'
            OR lower(genre) IN ('album', 'unknown', 'unknown genre', 'other', 'various',
                'various artists', 'genre', 'none', 'no genre', 'misc')
            OR (genre <> '' AND genre NOT GLOB '*[^0-9]*')
        );
        "#,
    ),
    // Pulse 4.0 removed the Liked idea, and this is where its remains leave the
    // schema. `loves` had already been dead weight since 3.0 dropped the
    // feature — never read, never written — and the view was the last thing
    // naming it.
    //
    // The trigger is dropped and recreated in the same step because it still
    // names `loves`: SQLite allows dropping a table that a trigger references,
    // which leaves a trigger that fails only when it fires. That is exactly the
    // kind of landmine this migration exists to clear.
    //
    // The seeded playlist is handled by `retire_liked_playlist` just before
    // this batch runs, because "is it empty?" is not worth expressing in SQL.
    (
        12,
        r#"
        DROP VIEW IF EXISTS liked_track_ids;
        DROP TRIGGER media_items_guard_user_data;
        DROP TABLE IF EXISTS love_deliveries;
        DROP TABLE IF EXISTS loves;
        DROP TABLE IF EXISTS remote_tracks;

        CREATE TRIGGER media_items_guard_user_data BEFORE DELETE ON media_items
        WHEN EXISTS (SELECT 1 FROM playlist_tracks WHERE track_id = OLD.id)
          OR EXISTS (
              SELECT 1 FROM lyrics
              WHERE track_id = OLD.id AND source NOT IN ('lrc', 'embedded')
          )
          OR EXISTS (SELECT 1 FROM plays WHERE track_id = OLD.id)
        BEGIN
            SELECT RAISE(ABORT, 'media item is referenced by user data');
        END;
        "#,
    ),
    // Scrobbling and Last.fm were removed in 4.0 — the whole idea, not just the
    // switches. `play_deliveries` was the queue of listens owed to a network
    // service; with no service to owe anything to, it is an empty table nothing
    // reads. `plays` stays: it is the listening history the app actually uses,
    // for play counts and recently-played.
    (
        13,
        r#"
        DROP TABLE IF EXISTS play_deliveries;
        "#,
    ),
    // Smart playlists. The spec is JSON rather than columns because a rule list
    // is a list: normalising it into a `smart_rules` table would mean a join per
    // edit and a migration every time a field or operator is added. Nothing is
    // materialised — the tracks are computed when the playlist is opened.
    (
        14,
        r#"
        CREATE TABLE smart_playlists (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            spec TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        "#,
    ),
    // Smart playlists are gone. They were never used, and the only thing that
    // was ever stored was the rule set — the tracks were always computed — so
    // dropping the table costs nothing but a feature nobody asked for twice.
    (15, "DROP TABLE IF EXISTS smart_playlists;"),
    // ReplayGain, in its own table rather than four more `tracks` columns: the
    // catalog row is what every list renders, and the gain is read exactly once
    // per track load. `tracks` is cleared and refilled by a scan, so the row
    // cascades away with it and is written again by the same scan.
    (
        16,
        r#"
        CREATE TABLE track_gain (
            track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
            track_gain REAL,
            track_peak REAL,
            album_gain REAL,
            album_peak REAL
        );
        "#,
    ),
];
