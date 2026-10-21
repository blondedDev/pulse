use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

struct TestLibrary;

impl pulse_remote::LibraryReader for TestLibrary {
    fn cover(&self, id: i64, size: pulse_remote::CoverSize) -> Option<Vec<u8>> {
        match (id, size) {
            (7, pulse_remote::CoverSize::Small) => Some(b"small-bytes".to_vec()),
            (7, pulse_remote::CoverSize::Large) => Some(b"large-bytes".to_vec()),
            _ => None,
        }
    }

    fn cover_original(&self, id: i64) -> Option<(Vec<u8>, String)> {
        match id {
            7 => Some((b"original-bytes".to_vec(), "image/jpeg".to_string())),
            _ => None,
        }
    }

    fn artists(&self) -> Vec<pulse_remote::ArtistEntry> {
        vec![pulse_remote::ArtistEntry {
            id: 1,
            name: "Smoke Artist".into(),
            track_count: 1,
            cover_ids: vec![7],
        }]
    }

    fn artist_detail(&self, artist_id: i64, full: bool) -> Option<pulse_remote::ArtistDetail> {
        if artist_id != 1 {
            return None;
        }
        let title = if full { "Full Album" } else { "Smoke Album" };
        Some(pulse_remote::ArtistDetail {
            id: 1,
            name: "Smoke Artist".into(),
            has_partial: true,
            albums: vec![pulse_remote::ArtistAlbum {
                album_id: Some(3),
                title: title.into(),
                year: Some(2024),
                cover_id: Some(7),
                partial: true,
                tracks: vec![pulse_remote::AlbumTrack {
                    id: 11,
                    title: "Smoke Track".into(),
                    track_number: Some(1),
                    disc_number: 1,
                    duration_ms: 1000,
                }],
            }],
        })
    }

    fn playlists(&self) -> Vec<pulse_remote::PlaylistEntry> {
        vec![pulse_remote::PlaylistEntry {
            id: 5,
            name: "Smoke Playlist".into(),
            track_count: 1,
        }]
    }

    fn playlist_detail(&self, playlist_id: i64) -> Option<pulse_remote::PlaylistDetail> {
        if playlist_id != 5 {
            return None;
        }
        Some(pulse_remote::PlaylistDetail {
            id: 5,
            name: "Smoke Playlist".into(),
            tracks: vec![pulse_remote::PlaylistTrack {
                id: 11,
                title: "Smoke Track".into(),
                artist: Some("Smoke Artist".into()),
                cover_id: Some(7),
                duration_ms: 1000,
            }],
        })
    }

    fn folders(&self, parent: Option<String>) -> Vec<pulse_remote::FolderEntry> {
        match parent.as_deref() {
            None => vec![pulse_remote::FolderEntry {
                path: "/music".into(),
                name: "music".into(),
                track_count: 2,
                album_count: 1,
            }],
            Some("/music") => vec![pulse_remote::FolderEntry {
                path: "/music/Rock".into(),
                name: "Rock".into(),
                track_count: 1,
                album_count: 1,
            }],
            Some(_) => Vec::new(),
        }
    }

    fn folder(&self, path: &str) -> Option<pulse_remote::FolderDetail> {
        if path != "/music/Rock" {
            return None;
        }
        Some(pulse_remote::FolderDetail {
            path: path.into(),
            name: "Rock".into(),
            tracks: vec![smoke_track()],
        })
    }

    fn all_tracks(&self) -> Vec<pulse_remote::PlaylistTrack> {
        vec![smoke_track()]
    }
}

fn smoke_track() -> pulse_remote::PlaylistTrack {
    pulse_remote::PlaylistTrack {
        id: 11,
        title: "Smoke Track".into(),
        artist: Some("Smoke Artist".into()),
        cover_id: Some(7),
        duration_ms: 1000,
    }
}

#[test]
fn serves_state_snapshot() {
    let addr: SocketAddr = ([127, 0, 0, 1], 18770).into();
    let (handle, rx) = pulse_remote::channel();
    let (commands, _command_rx) = pulse_remote::commands();
    let (_server, _ready) = pulse_remote::spawn(addr, rx, commands, Arc::new(TestLibrary));
    handle.publish(pulse_remote::PlayerState {
        has_track: true,
        title: Some("Smoke Track".into()),
        playing: true,
        volume: 0.5,
        ..Default::default()
    });

    let body = wait_for_state(addr);
    assert!(body.contains("\"v\":4"), "body: {body}");
    assert!(body.contains("Smoke Track"), "body: {body}");
    assert!(body.contains("\"playing\":true"), "body: {body}");
    assert!(body.contains("\"repeat\":\"off\""), "body: {body}");
    assert!(body.contains("\"volume\":0.5"), "body: {body}");

    let queue = try_get(addr, "/api/queue").expect("queue endpoint");
    assert_eq!(queue.trim(), "[]", "queue: {queue}");

    let small = try_get(addr, "/api/cover?id=7&size=small").expect("cover endpoint");
    assert_eq!(small, "small-bytes");
    let large = try_get(addr, "/api/cover?id=7").expect("cover endpoint");
    assert_eq!(large, "large-bytes");
    assert!(try_get(addr, "/api/cover?id=8").is_none());

    let artists = try_get(addr, "/api/artists").expect("artists endpoint");
    assert!(artists.contains("Smoke Artist"), "artists: {artists}");
    assert!(artists.contains("\"cover_ids\":[7]"), "artists: {artists}");

    let artist = try_get(addr, "/api/artist?id=1").expect("artist endpoint");
    assert!(artist.contains("Smoke Album"), "artist: {artist}");
    assert!(artist.contains("\"has_partial\":true"), "artist: {artist}");
    let artist_full = try_get(addr, "/api/artist?id=1&full=1").expect("artist endpoint");
    assert!(artist_full.contains("Full Album"), "artist: {artist_full}");
    assert!(try_get(addr, "/api/artist?id=2").is_none());

    let playlists = try_get(addr, "/api/playlists").expect("playlists endpoint");
    assert!(
        playlists.contains("Smoke Playlist"),
        "playlists: {playlists}"
    );

    let playlist = try_get(addr, "/api/playlist?id=5").expect("playlist endpoint");
    assert!(playlist.contains("Smoke Playlist"), "playlist: {playlist}");
    assert!(playlist.contains("Smoke Track"), "playlist: {playlist}");
    assert!(try_get(addr, "/api/playlist?id=6").is_none());

    // Folder browsing: roots, then one level down, then the folder itself.
    let roots = try_get(addr, "/api/folders").expect("folders endpoint");
    assert!(roots.contains("\"name\":\"music\""), "roots: {roots}");
    let children = try_get(addr, "/api/folders?parent=%2Fmusic").expect("folders endpoint");
    assert!(children.contains("\"name\":\"Rock\""), "children: {children}");
    let empty = try_get(addr, "/api/folders?parent=%2Fnowhere").expect("folders endpoint");
    assert_eq!(empty.trim(), "[]");

    let folder = try_get(addr, "/api/folder?path=%2Fmusic%2FRock").expect("folder endpoint");
    assert!(folder.contains("Smoke Track"), "folder: {folder}");
    assert!(try_get(addr, "/api/folder?path=%2Fnowhere").is_none());

    let tracks = try_get(addr, "/api/tracks").expect("tracks endpoint");
    assert!(tracks.contains("Smoke Track"), "tracks: {tracks}");
}

fn wait_for_state(addr: SocketAddr) -> String {
    for _ in 0..50 {
        if let Some(body) = try_get(addr, "/api/state") {
            return body;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("server did not respond on {addr}");
}

fn try_get(addr: SocketAddr, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    let (head, body) = buf.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200") {
        return None;
    }
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}
