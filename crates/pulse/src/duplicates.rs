//! One file per song, chosen by where it lives.
//!
//! The library happily holds the same recording three times — an mp3 in
//! `Albums/`, a FLAC in `FLAC Albums/`, an m4a in `Liked Songs/` — because each
//! is a real file. This module finds those groups and decides which copy to
//! keep from an ordered list of folders: the user says `FLAC Albums` beats
//! `Albums` beats `Liked Songs`, and the copy in the best folder survives.
//!
//! Three rules make it safe, and they are the whole contract:
//!
//! - **Only listed folders are touched.** The priority list defines both the
//!   ranking *and* the scope; a copy somewhere the user did not list is never
//!   deleted and never makes another copy deletable.
//! - **The best copy is never a candidate.** Only copies in a *strictly* worse
//!   folder are. Two copies in the same folder stay: there is no honest way to
//!   pick between them.
//! - **Cue pieces are never touched.** A cue track shares one audio file with
//!   its siblings, so "deleting the duplicate" would gut an album.
//!
//! The actual deletion lives in [`trash`], which moves files to the desktop
//! Trash rather than unlinking them.

use std::path::Path;

/// One file that could be a duplicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub track_id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    /// File size in bytes, or 0 when it could not be read.
    pub size: u64,
    pub is_cue: bool,
}

/// A group of copies of one song that the priority list can settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    /// Every copy at the best priority found. More than one means the list
    /// cannot separate them, and they all stay.
    pub keep: Vec<Candidate>,
    /// Copies in a strictly worse folder.
    pub remove: Vec<Candidate>,
}

impl Duplicate {
    pub fn title(&self) -> &str {
        &self.keep[0].title
    }

    pub fn artist(&self) -> &str {
        &self.keep[0].artist
    }
}

/// The whole result of a scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub duplicates: Vec<Duplicate>,
    /// Files that would be moved to the Trash.
    pub removable: Vec<Candidate>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.removable.is_empty()
    }

    pub fn removable_bytes(&self) -> u64 {
        self.removable.iter().map(|file| file.size).sum()
    }
}

/// Copies whose durations differ by more than this are different recordings.
///
/// A live take and a studio take of the same song share artist, title and album
/// but not their length; deleting one because of that would be the worst thing
/// this tool could do.
const DURATION_TOLERANCE_MS: i64 = 5_000;

/// Where `path` sits in the priority list, or `None` when it is not inside any
/// listed folder.
///
/// The *first* match wins, so a deeper folder listed on its own beats the
/// broader folder that also contains it: listing `FLAC Albums/` above
/// `Complete Music Collection/` ranks a FLAC file by the narrower entry.
pub fn priority_of(path: &str, priority: &[String]) -> Option<usize> {
    let file = Path::new(path);
    priority.iter().position(|folder| {
        let folder = Path::new(folder);
        !folder.as_os_str().is_empty() && file.starts_with(folder)
    })
}

fn identity(candidate: &Candidate) -> Option<(String, String, String)> {
    let title = music_library::normalize_tag(&candidate.title);
    if title.is_empty() {
        return None;
    }
    // An empty artist is allowed: plenty of single-track drops have no tags, and
    // the album and duration still tell them apart from everything else.
    Some((
        music_library::normalize_tag(&candidate.artist),
        title,
        music_library::normalize_tag(candidate.album.as_deref().unwrap_or("")),
    ))
}

/// Group by identity, then split each group into clusters of near-equal length.
fn clusters(candidates: &[Candidate]) -> Vec<Vec<Candidate>> {
    let mut by_identity: Vec<((String, String, String), Vec<Candidate>)> = Vec::new();
    for candidate in candidates {
        let Some(key) = identity(candidate) else {
            continue;
        };
        match by_identity.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, bucket)) => bucket.push(candidate.clone()),
            None => by_identity.push((key, vec![candidate.clone()])),
        }
    }

    let mut out = Vec::new();
    for (_, bucket) in by_identity {
        // Greedy by duration: each copy joins the first cluster it is close
        // enough to. Copies with no duration form a cluster of their own, keyed
        // by their album so they do not merge across records.
        let mut remaining: Vec<Vec<Candidate>> = Vec::new();
        for candidate in bucket {
            let joined = remaining.iter_mut().any(|cluster| {
                match (cluster[0].duration_ms, candidate.duration_ms) {
                    (Some(a), Some(b)) => (a - b).abs() <= DURATION_TOLERANCE_MS,
                    (None, None) => true,
                    _ => false,
                }
            });
            if joined {
                // Push into the cluster that accepted it.
                let index = remaining
                    .iter()
                    .position(|cluster| match (cluster[0].duration_ms, candidate.duration_ms)
                    {
                        (Some(a), Some(b)) => (a - b).abs() <= DURATION_TOLERANCE_MS,
                        (None, None) => true,
                        _ => false,
                    })
                    .expect("just matched");
                remaining[index].push(candidate);
            } else {
                remaining.push(vec![candidate]);
            }
        }
        out.extend(remaining);
    }
    out
}

/// Find the copies that the priority list can settle.
pub fn find(candidates: Vec<Candidate>, priority: &[String]) -> Report {
    // Only files inside a listed folder take part at all.
    let ranked: Vec<(usize, Candidate)> = candidates
        .into_iter()
        .filter(|candidate| !candidate.is_cue)
        .filter_map(|candidate| {
            priority_of(&candidate.path, priority).map(|rank| (rank, candidate))
        })
        .collect();

    let plain: Vec<Candidate> = ranked.iter().map(|(_, c)| c.clone()).collect();
    let mut report = Report::default();

    for cluster in clusters(&plain) {
        let mut best: Option<usize> = None;
        let mut keep = Vec::new();
        let mut remove = Vec::new();
        for (rank, candidate) in cluster
            .iter()
            .filter_map(|c| ranked.iter().find(|(_, r)| r == c))
        {
            match best {
                None => {
                    best = Some(*rank);
                    keep.push(candidate.clone());
                }
                Some(b) if *rank < b => {
                    // A better folder turned up: what was going to be kept is
                    // now a removal candidate.
                    best = Some(*rank);
                    remove.append(&mut keep);
                    keep.push(candidate.clone());
                }
                Some(b) if *rank == b => keep.push(candidate.clone()),
                Some(_) => remove.push(candidate.clone()),
            }
        }
        if !remove.is_empty() {
            report.removable.extend(remove.iter().cloned());
            report.duplicates.push(Duplicate { keep, remove });
        }
    }

    report.duplicates.sort_by(|a, b| {
        a.artist()
            .to_lowercase()
            .cmp(&b.artist().to_lowercase())
            .then(a.title().to_lowercase().cmp(&b.title().to_lowercase()))
    });
    report
}

/// Build the candidate list from the library.
///
/// One pass over the catalog plus two batch lookups (artists by item, album
/// titles), never a query per track. Sizes come from the filesystem because the
/// catalog does not store them for local files.
pub fn candidates(library: &crate::library_service::LibraryService) -> Vec<Candidate> {
    let tracks = library.all_tracks();
    let ids: Vec<i64> = tracks.iter().map(|track| track.id).collect();
    let artists = library.track_artists_map(&ids);
    let albums: std::collections::HashMap<i64, String> = library
        .albums()
        .into_iter()
        .map(|album| (album.id, album.title))
        .collect();

    tracks
        .into_iter()
        .map(|track| Candidate {
            track_id: track.id,
            size: std::fs::metadata(&track.path).map(|meta| meta.len()).unwrap_or(0),
            artist: artists
                .get(&track.id)
                .and_then(|names| names.first().cloned())
                .unwrap_or_default(),
            album: track.album_id.and_then(|id| albums.get(&id).cloned()),
            path: track.path,
            title: track.title,
            duration_ms: track.duration_ms,
            is_cue: track.is_cue,
        })
        .collect()
}

/// Move files to the desktop Trash.
///
/// Returns how many moved and the failures, because a partial result is a real
/// outcome here: one file can be on a drive that was unplugged halfway through.
pub fn trash(paths: &[String]) -> (usize, Vec<(String, String)>) {
    let mut moved = 0;
    let mut failures = Vec::new();
    for path in paths {
        match std::process::Command::new("gio")
            .arg("trash")
            .arg("--")
            .arg(path)
            .status()
        {
            Ok(status) if status.success() => moved += 1,
            Ok(status) => failures.push((path.clone(), format!("gio trash exited {status}"))),
            Err(err) => failures.push((path.clone(), err.to_string())),
        }
    }
    (moved, failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64, path: &str, title: &str, artist: &str, album: &str) -> Candidate {
        Candidate {
            track_id: id,
            path: path.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album: Some(album.to_string()),
            duration_ms: Some(180_000),
            size: 1000,
            is_cue: false,
        }
    }

    fn priority() -> Vec<String> {
        vec![
            "/music/FLAC Albums".to_string(),
            "/music/Albums".to_string(),
            "/music/Liked Songs".to_string(),
        ]
    }

    #[test]
    fn the_best_folder_wins_and_the_rest_are_removed() {
        let tracks = vec![
            candidate(1, "/music/Liked Songs/a.m4a", "Song", "Artist", "Record"),
            candidate(2, "/music/Albums/a.mp3", "Song", "Artist", "Record"),
            candidate(3, "/music/FLAC Albums/a.flac", "Song", "Artist", "Record"),
        ];

        let report = find(tracks, &priority());

        assert_eq!(report.duplicates.len(), 1);
        let duplicate = &report.duplicates[0];
        assert_eq!(duplicate.keep.len(), 1);
        assert_eq!(duplicate.keep[0].track_id, 3, "FLAC is the best folder");
        assert_eq!(duplicate.remove.len(), 2);
        assert_eq!(report.removable.len(), 2);
        assert_eq!(report.removable_bytes(), 2000);
    }

    #[test]
    fn a_folder_that_is_not_listed_is_never_touched() {
        let tracks = vec![
            candidate(1, "/music/Liked Songs/a.m4a", "Song", "Artist", "Record"),
            candidate(2, "/elsewhere/a.mp3", "Song", "Artist", "Record"),
        ];

        let report = find(tracks, &priority());

        assert!(report.is_empty(), "only one listed copy: nothing to settle");
    }

    #[test]
    fn a_folder_outside_the_list_cannot_make_a_listed_copy_deletable() {
        let tracks = vec![
            candidate(1, "/music/Liked Songs/a.m4a", "Song", "Artist", "Record"),
            candidate(2, "/music/FLAC Albums/a.flac", "Song", "Artist", "Record"),
            candidate(3, "/elsewhere/a.mp3", "Song", "Artist", "Record"),
        ];

        let report = find(tracks, &priority());

        assert_eq!(report.duplicates[0].keep[0].track_id, 2);
        assert_eq!(report.removable.len(), 1, "the unlisted copy stays put");
        assert_eq!(report.removable[0].track_id, 1);
    }

    #[test]
    fn two_copies_in_the_same_folder_are_both_kept() {
        let tracks = vec![
            candidate(1, "/music/Albums/a.mp3", "Song", "Artist", "Record"),
            candidate(2, "/music/Albums/a.flac", "Song", "Artist", "Record"),
            candidate(3, "/music/Liked Songs/a.m4a", "Song", "Artist", "Record"),
        ];

        let report = find(tracks, &priority());

        assert_eq!(report.duplicates[0].keep.len(), 2);
        assert_eq!(report.removable.len(), 1);
        assert_eq!(report.removable[0].track_id, 3);
    }

    #[test]
    fn different_lengths_are_different_recordings() {
        let mut tracks = vec![
            candidate(1, "/music/Albums/a.mp3", "Song", "Artist", "Record"),
            candidate(2, "/music/Liked Songs/a.m4a", "Song", "Artist", "Record"),
        ];
        // A live version, eight minutes long, against a three-minute studio cut.
        tracks[0].duration_ms = Some(180_000);
        tracks[1].duration_ms = Some(480_000);

        let report = find(tracks, &priority());

        assert!(report.is_empty());
    }

    #[test]
    fn cue_pieces_are_never_candidates() {
        let mut tracks = vec![
            candidate(1, "/music/Albums/a.flac", "Song", "Artist", "Record"),
            candidate(2, "/music/Liked Songs/a.m4a", "Song", "Artist", "Record"),
        ];
        tracks[1].is_cue = true;

        let report = find(tracks, &priority());

        assert!(report.is_empty());
    }

    #[test]
    fn different_albums_are_not_duplicates() {
        let tracks = vec![
            candidate(1, "/music/Albums/a.mp3", "Intro", "Artist", "One"),
            candidate(2, "/music/Liked Songs/a.m4a", "Intro", "Artist", "Two"),
        ];

        let report = find(tracks, &priority());

        assert!(report.is_empty());
    }

    #[test]
    fn the_first_matching_folder_decides_the_rank() {
        let priority = vec![
            "/music/FLAC Albums".to_string(),
            "/music".to_string(),
        ];
        assert_eq!(
            priority_of("/music/FLAC Albums/a.flac", &priority),
            Some(0)
        );
        assert_eq!(priority_of("/music/Albums/a.mp3", &priority), Some(1));
        assert_eq!(priority_of("/other/a.mp3", &priority), None);
    }
}
