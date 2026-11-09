use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cover_search::Candidate;
use ureq::Agent;

const ART_SIZE: u32 = 512;
const MAX_URL_LEN: usize = 254;
/// Results to ask iTunes for. One is not enough: the first hit for an
/// "artist album" query is regularly a tribute, karaoke or deluxe edition.
const SEARCH_LIMIT: usize = 10;
/// How long a lookup is trusted. Nothing lasts forever, because a wrong first
/// answer used to stay in this cache until the file was deleted by hand.
const CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Words that mark a release as "the same songs, someone else's recording".
const NOT_THE_ALBUM: [&str; 8] = [
    "karaoke",
    "tribute",
    "made famous",
    "in the style of",
    "cover version",
    "covers",
    "lullaby",
    "8 bit",
];
/// Edition words dropped before comparing titles, so "Dummy (Deluxe Edition)"
/// still matches "Dummy".
const EDITION_WORDS: [&str; 11] = [
    "deluxe",
    "edition",
    "remaster",
    "remastered",
    "expanded",
    "anniversary",
    "bonus",
    "special",
    "explicit",
    "version",
    "remasterd",
];

pub struct ArtCache {
    path: PathBuf,
    agent: Agent,
    map: HashMap<String, Entry>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Entry {
    url: String,
    at: u64,
}

impl ArtCache {
    pub fn load(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let agent = Agent::new_with_config(
            Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(10)))
                .timeout_recv_body(Some(Duration::from_secs(10)))
                .build(),
        );
        Self { path, agent, map }
    }

    /// A cover URL for the album, or `None`.
    ///
    /// `album_artist` is preferred over the track artist: on a compilation or
    /// a featured track, the track artist is not who the album is filed under,
    /// and asking for the wrong artist is how a lookup lands on a stranger's
    /// album.
    pub fn resolve(
        &mut self,
        album_artist: Option<&str>,
        artist: &str,
        album: Option<&str>,
    ) -> Option<String> {
        let album = album?;
        let query_artist = album_artist
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(artist);
        if query_artist.is_empty() || album.trim().is_empty() {
            return None;
        }

        // `v2` because the old cache stored the unverified first hit forever.
        let key = format!("v2|{query_artist}|{album}");
        let now = unix_now();
        if let Some(entry) = self.map.get(&key) {
            if now.saturating_sub(entry.at) < CACHE_TTL.as_secs() {
                return non_empty(&entry.url);
            }
        }

        let found = match lookup(&self.agent, query_artist, album) {
            Ok(found) => found.unwrap_or_default(),
            // A transport error is not a miss: leave nothing cached so the next
            // track (or the next play) tries again.
            Err(()) => return None,
        };
        self.map.insert(
            key,
            Entry {
                url: found.clone(),
                at: now,
            },
        );
        self.persist();
        non_empty(&found)
    }

    fn persist(&self) {
        let Ok(json) = serde_json::to_string(&self.map) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn non_empty(url: &str) -> Option<String> {
    (!url.is_empty()).then(|| url.to_string())
}

fn lookup(agent: &Agent, artist: &str, album: &str) -> Result<Option<String>, ()> {
    let candidates =
        cover_search::itunes::search(agent, artist, album, SEARCH_LIMIT).map_err(|_| ())?;
    Ok(choose(artist, album, &candidates))
}

/// The best candidate, or nothing.
///
/// Returning `None` is deliberate: no cover is better than someone else's
/// cover. The tooltip says "Album", and a wrong picture under the right album
/// name is worse than an empty slot.
fn choose(artist: &str, album: &str, candidates: &[Candidate]) -> Option<String> {
    let want_artist = normalize(artist);
    let want_album = normalize(album);
    let mut best: Option<(i32, String)> = None;

    for candidate in candidates {
        let found_artist = normalize(&candidate.artist);
        let found_album = normalize(&candidate.album);
        if is_decoy(&found_artist) || is_decoy(&found_album) {
            continue;
        }
        let Some(album_score) = text_score(&want_album, &found_album, 4, 2) else {
            continue;
        };
        let Some(artist_score) = text_score(&want_artist, &found_artist, 3, 1) else {
            continue;
        };
        let url = candidate.art_url(ART_SIZE);
        if url.len() > MAX_URL_LEN {
            continue;
        }
        let score = album_score + artist_score;
        if best.as_ref().is_none_or(|(current, _)| score > *current) {
            best = Some((score, url));
        }
    }

    best.map(|(_, url)| url)
}

fn text_score(want: &str, candidate: &str, exact: i32, partial: i32) -> Option<i32> {
    if want.is_empty() || candidate.is_empty() {
        return None;
    }
    if want == candidate {
        return Some(exact);
    }
    // Containment only counts once there is enough string to mean something;
    // otherwise "a" matches half the catalogue.
    let shorter = want.len().min(candidate.len());
    if shorter >= 4 && (want.contains(candidate) || candidate.contains(want)) {
        return Some(partial);
    }
    None
}

fn is_decoy(normalized: &str) -> bool {
    NOT_THE_ALBUM
        .iter()
        .any(|word| normalized.contains(word))
}

fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        let lower = fold(c);
        if lower == '&' {
            out.push(' ');
        } else if lower.is_alphanumeric() {
            out.push(lower);
        } else {
            out.push(' ');
        }
    }
    // Tokenise, drop edition words, rejoin.
    let tokens: Vec<&str> = out
        .split_whitespace()
        .filter(|token| !EDITION_WORDS.contains(token))
        .collect();
    tokens.join(" ")
}

/// Lower-case and strip the accents that appear in album credits.
fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ß' => 's',
        'æ' => 'a',
        'œ' => 'o',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_path(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pulse-discord-art-{}-{tag}-{:?}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn candidates(json: &str) -> Vec<Candidate> {
        cover_search::itunes::parse(json).unwrap()
    }

    #[test]
    fn cache_hit_and_negative_hit_skip_network() {
        let path = cache_path("hit");
        let now = unix_now();
        std::fs::write(
            &path,
            format!(
                r#"{{"v2|A|Hit":{{"url":"https://img/512x512bb.jpg","at":{now}}},
                    "v2|A|Miss":{{"url":"","at":{now}}}}}"#
            ),
        )
        .unwrap();

        let mut cache = ArtCache::load(path.clone());
        assert_eq!(
            cache.resolve(None, "A", Some("Hit")).as_deref(),
            Some("https://img/512x512bb.jpg")
        );
        assert_eq!(cache.resolve(None, "A", Some("Miss")), None);
        assert_eq!(cache.resolve(None, "A", None), None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_first_result_is_ignored_when_it_is_a_karaoke_record() {
        // The real failure mode: iTunes ranks a karaoke version first.
        let found = candidates(
            r#"{"results":[
                {"artistName":"Karaoke Hits","collectionName":"Dummy (Karaoke Version)",
                 "artworkUrl100":"https://is1.mzstatic.com/k/100x100bb.jpg"},
                {"artistName":"Portishead","collectionName":"Dummy",
                 "artworkUrl100":"https://is1.mzstatic.com/p/100x100bb.jpg"}
            ]}"#,
        );
        assert_eq!(
            choose("Portishead", "Dummy", &found).as_deref(),
            Some("https://is1.mzstatic.com/p/512x512bb.jpg")
        );
    }

    #[test]
    fn a_completely_different_album_is_not_a_fallback() {
        let found = candidates(
            r#"{"results":[
                {"artistName":"Portishead","collectionName":"Third",
                 "artworkUrl100":"https://is1.mzstatic.com/t/100x100bb.jpg"}
            ]}"#,
        );
        assert_eq!(choose("Portishead", "Dummy", &found), None);
    }

    #[test]
    fn edition_suffixes_and_accents_still_match() {
        let found = candidates(
            r#"{"results":[
                {"artistName":"Beyoncé","collectionName":"Lemonade (Deluxe Edition)",
                 "artworkUrl100":"https://is1.mzstatic.com/l/100x100bb.jpg"}
            ]}"#,
        );
        assert_eq!(
            choose("Beyonce", "Lemonade", &found).as_deref(),
            Some("https://is1.mzstatic.com/l/512x512bb.jpg")
        );
    }

    #[test]
    fn the_album_artist_is_what_gets_looked_up() {
        // A track on a compilation: the track artist is "Guest", the album is
        // credited to "Various Artists". The query has to use the latter.
        let found = candidates(
            r#"{"results":[
                {"artistName":"Various Artists","collectionName":"Late Night Tales",
                 "artworkUrl100":"https://is1.mzstatic.com/v/100x100bb.jpg"}
            ]}"#,
        );
        assert_eq!(
            choose("Various Artists", "Late Night Tales", &found).as_deref(),
            Some("https://is1.mzstatic.com/v/512x512bb.jpg")
        );
        assert_eq!(choose("Guest", "Late Night Tales", &found), None);
    }
}
