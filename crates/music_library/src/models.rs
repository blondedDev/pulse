#[derive(Debug, Clone)]
pub struct CoverArt {
    pub id: i64,
    pub small: Vec<u8>,
    pub large: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub sort_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub cover_art_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub album_id: Option<i64>,
    pub track_number: Option<i32>,
    pub disc_number: i32,
    pub duration_ms: Option<i64>,
    pub year: Option<i32>,
    pub cover_art_id: Option<i64>,
    pub start_offset_ms: i32,
    #[serde(default)]
    pub bitrate: Option<u32>,
    #[serde(default)]
    pub is_cue: bool,
    #[serde(default = "available_by_default")]
    pub available: bool,
}

fn available_by_default() -> bool {
    true
}

impl Track {
    /// Prefix older builds wrote for a track that lived on a media server.
    ///
    /// Media servers were removed in 3.0, but a library.db written before that
    /// can still hold such a row. Those are not files, so they must not be
    /// handed to the tag editor or the decoder.
    const REMOTE_LOCATOR_PREFIX: &'static str = "pawse-source://";

    pub fn local_file(&self) -> Option<&std::path::Path> {
        (!self.path.starts_with(Self::REMOTE_LOCATOR_PREFIX))
            .then(|| std::path::Path::new(&self.path))
    }

    /// The single file this track owns, as opposed to the image a cue track
    /// shares with its siblings.
    pub fn own_file(&self) -> Option<&std::path::Path> {
        self.local_file().filter(|_| !self.is_cue)
    }
}

/// ReplayGain as read from a file's tags.
///
/// Both scales live here because both are written in the wild: ReplayGain 2.0
/// (`REPLAYGAIN_*` in dB against -18 LUFS) and R128/Opus (`R128_*` in Q7.8 LU
/// against -23 LUFS). The indexer normalises R128 into dB at read time, so
/// everything downstream is one unit.
///
/// Peaks are linear sample peaks (0..=1) and are only used to stop a positive
/// gain from clipping.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

/// Anything louder than this is treated as a broken tag. Real gain values sit
/// within ±20 dB; a stray "0.5" meant as a peak would otherwise blow the mix up.
const MAX_GAIN_DB: f32 = 24.;

impl ReplayGain {
    pub fn is_empty(&self) -> bool {
        self.track_gain.is_none() && self.album_gain.is_none()
    }

    /// The gain to apply, in dB, before clipping protection.
    ///
    /// Album mode falls back to the track value (and vice versa) because plenty
    /// of rips carry only one of the two.
    pub fn gain_db(&self, prefer_album: bool) -> Option<f32> {
        let (first, second) = if prefer_album {
            (self.album_gain, self.track_gain)
        } else {
            (self.track_gain, self.album_gain)
        };
        first.or(second).filter(|db| db.is_finite() && *db <= MAX_GAIN_DB)
    }

    /// The linear multiplier to put in the signal path.
    ///
    /// `limit_clipping` caps a positive gain at the headroom the peak allows, so
    /// a track tagged +6 dB with a 0.9 peak is turned down to +0.9 dB instead of
    /// clipping. A gain that would only turn the track down is never limited.
    pub fn linear(&self, prefer_album: bool, limit_clipping: bool) -> f32 {
        let Some(db) = self.gain_db(prefer_album) else {
            return 1.0;
        };
        let db = if limit_clipping && db > 0.0 {
            let peak = if prefer_album {
                self.album_peak.or(self.track_peak)
            } else {
                self.track_peak
            };
            match peak {
                Some(peak) if peak > 0.0 && peak.is_finite() => {
                    db.min(-20.0 * peak.log10()).max(0.0)
                }
                _ => db,
            }
        } else {
            db
        };
        10f32.powf(db / 20.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumSummary {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub cover_art_id: Option<i64>,
    pub artist_name: String,
    pub artist_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumSearchEntry {
    pub album_id: i64,
    pub haystack: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistGrouping {
    TrackArtist,
    #[default]
    AlbumArtist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistSummary {
    pub id: i64,
    pub name: String,
    pub sort_name: String,
    pub track_count: i64,
}

/// A directory in the library's folder hierarchy.
///
/// Folder structure is not stored: it is derived from the absolute paths in
/// `tracks`, anchored on the configured local roots. So a folder exists only
/// for as long as something under it does, and `track_count` is the number of
/// tracks reachable below it at any depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSummary {
    /// Absolute path of the directory, without a trailing separator.
    pub path: String,
    /// Last path component, for display.
    pub name: String,
    /// Tracks below this folder, at any depth.
    pub track_count: i64,
    /// Distinct albums below this folder.
    pub album_count: i64,
}

/// Splits a folder path into the folder above it and its own name.
///
/// `separator` has to be the one the repository joins with, which is
/// [`std::path::MAIN_SEPARATOR`]: folder paths are derived from the absolute
/// paths the indexer stored, and those are host-native, so they carry a `\`
/// on Windows. Splitting a Windows folder path on a hardcoded `/` there would
/// hand back the whole path as the folder's own name.
///
/// Returns an empty parent for a top-level folder. Both halves are still meant
/// to go back to the repository, which is the only thing that knows how to look
/// a folder up.
pub fn split_folder_path(path: &str, separator: char) -> (&str, &str) {
    let Some(ix) = path.rfind(separator) else {
        return ("", path);
    };
    let (head, name) = (&path[..ix], &path[ix + separator.len_utf8()..]);
    // A drive root is the separator right after its designator. Without it the
    // parent would come back as `D:`, which is a drive-relative path rather
    // than a directory, and would match no folder at all.
    if head.ends_with(':') {
        return (&path[..ix + separator.len_utf8()], name);
    }
    (head, name)
}

#[cfg(test)]
mod tests {
    use super::split_folder_path;
    use super::ReplayGain;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn no_tags_means_unity() {
        let rg = ReplayGain::default();
        assert!(rg.is_empty());
        assert_eq!(rg.gain_db(false), None);
        assert_eq!(rg.linear(false, true), 1.0);
    }

    #[test]
    fn a_gain_becomes_a_linear_multiplier() {
        let rg = ReplayGain {
            track_gain: Some(-6.0),
            ..ReplayGain::default()
        };
        // -6 dB is a half.
        assert!(close(rg.linear(false, false), 0.501));
    }

    #[test]
    fn album_and_track_modes_fall_back_to_each_other() {
        let only_track = ReplayGain {
            track_gain: Some(-3.0),
            ..ReplayGain::default()
        };
        let only_album = ReplayGain {
            album_gain: Some(-3.0),
            ..ReplayGain::default()
        };
        assert_eq!(only_track.gain_db(true), Some(-3.0));
        assert_eq!(only_album.gain_db(false), Some(-3.0));

        let both = ReplayGain {
            track_gain: Some(-3.0),
            album_gain: Some(-5.0),
            ..ReplayGain::default()
        };
        assert_eq!(both.gain_db(false), Some(-3.0));
        assert_eq!(both.gain_db(true), Some(-5.0));
    }

    #[test]
    fn clipping_protection_caps_a_positive_gain_at_the_peak_headroom() {
        let rg = ReplayGain {
            track_gain: Some(6.0),
            track_peak: Some(0.9),
            ..ReplayGain::default()
        };
        // 20*log10(1/0.9) is about 0.915 dB, not 6.
        let limited_db = 20.0 * rg.linear(false, true).log10();
        assert!(close(limited_db, 0.915));
        // Without protection the full +6 dB is applied.
        let full_db = 20.0 * rg.linear(false, false).log10();
        assert!(close(full_db, 6.0));
    }

    #[test]
    fn clipping_protection_never_turns_a_negative_gain_down_further() {
        let rg = ReplayGain {
            track_gain: Some(-8.0),
            track_peak: Some(0.2),
            ..ReplayGain::default()
        };
        // 0.2 peak would allow up to +14 dB; a negative gain is already safe.
        let db = 20.0 * rg.linear(false, true).log10();
        assert!(close(db, -8.0));
    }

    #[test]
    fn an_implausible_gain_tag_is_ignored() {
        let rg = ReplayGain {
            track_gain: Some(90.0),
            ..ReplayGain::default()
        };
        assert_eq!(rg.gain_db(false), None);
        assert_eq!(rg.linear(false, false), 1.0);
    }
    use std::path::MAIN_SEPARATOR;

    /// Covers the separator this machine's repository joins folder paths with.
    #[test]
    fn split_folder_path_works_on_the_host_separator() {
        let (deep, parent) = if MAIN_SEPARATOR == '/' {
            ("/music/Rock/Nightwish", "/music/Rock")
        } else {
            (r"C:\music\Rock\Nightwish", r"C:\music\Rock")
        };
        assert_eq!(
            split_folder_path(deep, MAIN_SEPARATOR),
            (parent, "Nightwish")
        );
        assert_eq!(split_folder_path("Rock", MAIN_SEPARATOR), ("", "Rock"));
    }

    /// The split follows the separator it is handed, not whichever one the
    /// machine running the test happens to use.
    #[test]
    fn split_folder_path_follows_the_separator_it_is_given() {
        assert_eq!(split_folder_path("/music/Rock", '/'), ("/music", "Rock"));
        assert_eq!(
            split_folder_path(r"C:\music\Rock", '\\'),
            (r"C:\music", "Rock")
        );
        assert_eq!(
            split_folder_path(r"C:\music\Rock\Nightwish", '\\'),
            (r"C:\music\Rock", "Nightwish")
        );
        // The other separator is just an ordinary character here.
        assert_eq!(split_folder_path("/music/Rock", '\\'), ("", "/music/Rock"));
        assert_eq!(
            split_folder_path(r"C:\music\Rock", '/'),
            ("", r"C:\music\Rock")
        );
    }

    /// A Windows drive root keeps its separator, so the folder below it is
    /// `D:\Rock` and never `D:Rock`.
    #[test]
    fn split_folder_path_handles_a_drive_root() {
        assert_eq!(split_folder_path("D:\\Rock", '\\'), ("D:\\", "Rock"));
    }

    #[test]
    fn split_folder_path_keeps_a_trailing_separator_from_hiding_the_name() {
        assert_eq!(split_folder_path("/music/Rock/", '/'), ("/music/Rock", ""));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSummary {
    pub id: i64,
    pub kind: String,
    pub uri: String,
    pub enabled: bool,
    pub available: bool,
    pub track_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFolder {
    pub path: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewTrack {
    pub path: String,
    pub title: Option<String>,
    pub album_title: Option<String>,
    pub artist_names: Vec<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    pub duration_ms: Option<u64>,
    pub cover_art_id: Option<i64>,
    pub start_offset_ms: Option<u64>,
    pub bitrate: Option<u32>,
}

/// The canonical `lyrics.source` tags and the one place that classifies a source
/// as disk-derived (re-read on rescan) vs. network (must survive a rescan).
pub mod lyrics_source {
    pub const LRC: &str = "lrc";
    pub const EMBEDDED: &str = "embedded";
    pub const LRCLIB: &str = "lrclib";

    pub fn is_disk_derived(source: &str) -> bool {
        matches!(source, LRC | EMBEDDED)
    }

    pub fn order(prefer_lrclib: bool) -> [&'static str; 3] {
        if prefer_lrclib {
            [LRCLIB, LRC, EMBEDDED]
        } else {
            [LRC, EMBEDDED, LRCLIB]
        }
    }

    pub fn rank(source: &str, prefer_lrclib: bool) -> usize {
        order(prefer_lrclib)
            .iter()
            .position(|known| *known == source)
            .unwrap_or(usize::MAX)
    }

    pub fn choices(
        variants: &[super::StoredLyrics],
        prefer_lrclib: bool,
    ) -> Vec<&super::StoredLyrics> {
        let mut usable: Vec<&super::StoredLyrics> = variants
            .iter()
            .filter(|v| !v.not_found && !v.text.trim().is_empty())
            .collect();
        usable.sort_by_key(|v| rank(&v.source, prefer_lrclib));
        let mut distinct: Vec<&super::StoredLyrics> = Vec::with_capacity(usable.len());
        for variant in usable {
            if !distinct
                .iter()
                .any(|kept| kept.text.trim() == variant.text.trim())
            {
                distinct.push(variant);
            }
        }
        distinct
    }

    pub fn pick<'a>(
        variants: &'a [super::StoredLyrics],
        prefer_lrclib: bool,
        chosen: Option<&str>,
    ) -> Option<&'a super::StoredLyrics> {
        let choices = choices(variants, prefer_lrclib);
        chosen
            .and_then(|source| choices.iter().find(|v| v.source == source))
            .or_else(|| choices.first())
            .copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanLyrics {
    pub text: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLyrics {
    pub source: String,
    pub text: String,
    /// A remote lookup ran and found nothing: `text` is empty and the UI must
    /// not re-search.
    pub not_found: bool,
}

/// A track ready for batched scan insertion. Unlike [`NewTrack`], the cover is
/// referenced by content hash (resolved to a `cover_art_id` by the writer's
/// in-memory cache) rather than by id, so the parse workers never touch the DB.
///
/// No `Eq`: the ReplayGain values are floats.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScanTrack {
    pub path: String,
    pub title: Option<String>,
    pub album_title: Option<String>,
    pub artist_names: Vec<String>,
    pub album_artist_names: Vec<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub duration_ms: Option<u64>,
    pub cover_hash: Option<String>,
    pub start_offset_ms: Option<u64>,
    pub bitrate: Option<u32>,
    pub replaygain: ReplayGain,
    pub is_cue: bool,
    pub lyrics: Vec<ScanLyrics>,
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackArtist {
    pub track_id: i64,
    pub artist_id: i64,
    pub role: String,
    pub credited_as: Option<String>,
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub track_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayTally {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub plays: u32,
    pub qualified_plays: u32,
    pub last_played: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentPlay {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackListing {
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub album_id: Option<i64>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<i32>,
    pub plays: u32,
    pub last_played: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlay {
    pub track_id: Option<i64>,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<u64>,
    pub played_secs: Option<u64>,
    pub started_at: u64,
    pub qualified: bool,
}

/// What a library health check found.
///
/// A report, not a repair: it counts things and says so. Anything it flags is
/// either harmless (an orphaned catalog row is invisible to the user) or needs a
/// human decision, so nothing here deletes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    /// `PRAGMA integrity_check` — `ok`, or the first problem it reported.
    pub integrity: String,
    /// Rows `PRAGMA foreign_key_check` complains about.
    pub foreign_key_violations: i64,
    /// Playlist entries pointing at a track that no longer exists.
    pub dangling_playlist_entries: i64,
    /// Catalog rows with neither a track row nor a present binding.
    pub orphaned_catalog_rows: i64,
}

/// One line of a ranked list: a top artist, album or track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatEntry {
    /// The artist, album or track title.
    pub name: String,
    /// The artist, for album and track entries.
    pub subtitle: Option<String>,
    pub plays: u64,
    pub seconds: u64,
}

/// Listening history, summarised.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListeningStats {
    pub plays: u64,
    pub seconds: u64,
    pub artists: u64,
    pub albums: u64,
    pub tracks: u64,
    /// Distinct days with at least one play.
    pub days: u64,
    pub top_artists: Vec<StatEntry>,
    pub top_albums: Vec<StatEntry>,
    pub top_tracks: Vec<StatEntry>,
}

impl ListeningStats {
    pub fn is_empty(&self) -> bool {
        self.plays == 0
    }

    /// Listening time in whole hours — the number people actually quote.
    pub fn hours(&self) -> u64 {
        self.seconds / 3600
    }
}

impl IntegrityReport {
    /// True when nothing at all was found.
    pub fn is_clean(&self) -> bool {
        self.integrity == "ok"
            && self.foreign_key_violations == 0
            && self.dangling_playlist_entries == 0
            && self.orphaned_catalog_rows == 0
    }
}
