use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use music_library::Track;
use gpui::SharedString;
use rand::seq::SliceRandom;

use crate::settings_store::QueueRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

/// Where the current queue came from. Used by the UI to decide whether
/// per-track "remove from playlist" controls should be visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueSource {
    #[default]
    Unknown,
    Playlist(i64),
    AllTracks,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

pub struct PlaybackQueue {
    tracks: Vec<Rc<Track>>,
    original_order: Option<Vec<Rc<Track>>>,
    current_index: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
    source: QueueSource,
    /// Where the queue came from, for display: an album title, a playlist name,
    /// a folder name. `None` for a queue with no nameable origin, which is what
    /// a hand-built queue is.
    source_name: Option<SharedString>,
    custom: bool,
    /// Where the next "play next" insert goes: just past the tracks already
    /// queued that way.
    ///
    /// Queued tracks are spliced straight into `tracks`, so without this every
    /// insert landed at `current_index + 1` and pushed the previous one down —
    /// queueing three songs played them in reverse. Spotify plays them in the
    /// order you queued them, and so does this.
    queue_tail: Option<usize>,
}

pub struct QueueRestore {
    pub tracks: Vec<Rc<Track>>,
    pub original_order: Option<Vec<Rc<Track>>>,
    pub current_index: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub source: QueueSource,
    pub source_name: Option<SharedString>,
    pub custom: bool,
}

impl Default for PlaybackQueue {
    fn default() -> Self {
        Self::new()
    }
}

pub enum PreviousAction<'a> {
    SeekToStart,
    PreviousTrack(&'a Track),
}

/// Outcome of removing a queue entry so the caller can keep the audio engine
/// in sync when the currently-playing track is the one removed.
pub enum RemoveOutcome {
    /// A non-current track was removed (or index was out of range). The engine
    /// keeps playing untouched — only the view needs a refresh.
    Unaffected,
    /// The currently-playing track was removed; play this track, which has
    /// shifted into the same index.
    PlayNext(Rc<Track>),
    /// The currently-playing track was removed and nothing remains after it.
    Stopped,
}

impl PlaybackQueue {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            original_order: None,
            current_index: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            source: QueueSource::Unknown,
            source_name: None,
            custom: false,
            queue_tail: None,
        }
    }

    pub fn set_tracks(&mut self, tracks: Vec<Rc<Track>>) {
        self.set_tracks_with_source(tracks, QueueSource::Unknown);
    }

    pub fn set_tracks_with_source(&mut self, tracks: Vec<Rc<Track>>, source: QueueSource) {
        self.tracks = playable(tracks);
        self.original_order = None;
        self.current_index = None;
        self.queue_tail = None;
        self.source = source;
        self.source_name = None;
        self.custom = false;
        if self.shuffle {
            self.apply_shuffle();
        }
    }

    pub fn clear(&mut self) {
        self.set_tracks(Vec::new());
    }

    /// Set tracks and immediately mark `index` as the current position,
    /// so that `apply_shuffle` anchors the clicked track to slot 0 when
    /// shuffle is enabled. Callers pass the index into the *natural* order.
    /// When `index` is in bounds, `current_track()` returns the intended
    /// track after this call; otherwise it returns `None`.
    pub fn set_tracks_and_play_at(
        &mut self,
        tracks: Vec<Rc<Track>>,
        index: usize,
        source: QueueSource,
    ) -> Option<&Rc<Track>> {
        self.original_order = None;
        self.source = source;
        self.source_name = None;
        self.custom = false;
        self.current_index = tracks
            .get(index)
            .filter(|track| track.available)
            .map(|_| tracks[..index].iter().filter(|t| t.available).count());
        self.tracks = playable(tracks);
        if self.shuffle {
            self.apply_shuffle();
        }
        self.current_index.and_then(|i| self.tracks.get(i))
    }

    pub fn source(&self) -> QueueSource {
        self.source
    }

    pub fn is_custom(&self) -> bool {
        self.custom
    }

    pub fn set_source(&mut self, source: QueueSource) {
        self.source = source;
        self.source_name = None;
    }

    /// Set the source and the name to show for it ("Album", then its title).
    pub fn set_source_named(&mut self, source: QueueSource, name: Option<SharedString>) {
        self.source = source;
        self.source_name = name;
    }

    pub fn source_name(&self) -> Option<SharedString> {
        self.source_name.clone()
    }

    /// Replace the queue's track list with a fresh snapshot, preserving the
    /// currently-playing track by id. Used when the playlist backing the
    /// queue has new tracks added/reordered. When shuffle is on, the new
    /// snapshot becomes the new `original_order` and is reshuffled so
    /// `set_shuffle(false)` can still restore a meaningful order later.
    pub fn refresh_keeping_current(&mut self, new_tracks: Vec<Rc<Track>>) {
        let current_id = self.current_track().map(|t| t.id);
        self.tracks = playable(new_tracks);
        self.custom = false;
        self.current_index = current_id.and_then(|id| self.tracks.iter().position(|t| t.id == id));
        if self.shuffle {
            self.apply_shuffle();
        } else {
            self.original_order = None;
        }
    }

    pub fn remap_to_fresh_tracks(
        &mut self,
        fresh: &std::collections::HashMap<(String, i32), Rc<Track>>,
    ) {
        let current_key = self
            .current_index
            .and_then(|i| self.tracks.get(i))
            .map(|t| (t.path.clone(), t.start_offset_ms));

        let remap = |list: &[Rc<Track>]| -> Vec<Rc<Track>> {
            list.iter()
                .filter_map(|old| {
                    let key = (old.path.clone(), old.start_offset_ms);
                    match fresh.get(&key) {
                        Some(new) => Some(new.clone()),
                        None if Some(&key) == current_key.as_ref() => Some(old.clone()),
                        None => None,
                    }
                })
                .collect()
        };

        self.tracks = remap(&self.tracks);
        if let Some(orig) = self.original_order.as_ref() {
            self.original_order = Some(remap(orig));
        }

        self.current_index = current_key.and_then(|(path, off)| {
            self.tracks
                .iter()
                .position(|t| t.path == path && t.start_offset_ms == off)
        });
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn play_track_at(&mut self, index: usize) -> Option<&Rc<Track>> {
        if index < self.tracks.len() {
            self.current_index = Some(index);
            self.tracks.get(index)
        } else {
            None
        }
    }

    pub fn next_track(&mut self) -> Option<&Rc<Track>> {
        let current = self.current_index?;

        if let RepeatMode::One = self.repeat {
            return self.tracks.get(current);
        }

        let next = current + 1;
        if next < self.tracks.len() {
            self.current_index = Some(next);
            return self.tracks.get(next);
        }

        match self.repeat {
            RepeatMode::All if !self.tracks.is_empty() => {
                self.current_index = Some(0);
                self.tracks.first()
            }
            _ => {
                self.current_index = None;
                None
            }
        }
    }

    pub fn skip_to_next(&mut self) -> Option<&Rc<Track>> {
        if !self.has_next() {
            return None;
        }
        self.next_track()
    }

    pub fn previous(&mut self, position_secs: f32) -> PreviousAction<'_> {
        if position_secs > 3.0 {
            return PreviousAction::SeekToStart;
        }

        match self.current_index {
            Some(current) if current > 0 => {
                self.current_index = Some(current - 1);
                PreviousAction::PreviousTrack(self.tracks.get(current - 1).unwrap().as_ref())
            }
            _ => PreviousAction::SeekToStart,
        }
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current_index
            .and_then(|i| self.tracks.get(i))
            .map(|rc| rc.as_ref())
    }

    pub fn has_next(&self) -> bool {
        match self.current_index {
            Some(current) => {
                current + 1 < self.tracks.len()
                    || matches!(self.repeat, RepeatMode::All | RepeatMode::One)
                        && !self.tracks.is_empty()
            }
            None => false,
        }
    }

    pub fn has_previous(&self) -> bool {
        self.current_index.is_some()
    }

    /// Like `next_track` but does NOT advance the index — pure peek.
    pub fn peek_next(&self) -> Option<&Rc<Track>> {
        let current = self.current_index?;

        if let RepeatMode::One = self.repeat {
            return self.tracks.get(current);
        }

        let next = current + 1;
        if next < self.tracks.len() {
            return self.tracks.get(next);
        }

        match self.repeat {
            RepeatMode::All if !self.tracks.is_empty() => self.tracks.first(),
            _ => None,
        }
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    pub fn tracks_vec(&self) -> Vec<Rc<Track>> {
        self.tracks.clone()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn set_shuffle(&mut self, enabled: bool) {
        if self.shuffle == enabled {
            return;
        }
        self.shuffle = enabled;
        if enabled {
            self.apply_shuffle();
        } else {
            self.restore_original_order();
        }
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    pub fn original_order_vec(&self) -> Option<Vec<Rc<Track>>> {
        self.original_order.clone()
    }

    pub fn restore(&mut self, state: QueueRestore) {
        self.tracks = state.tracks;
        self.original_order = state.original_order;
        self.current_index = state.current_index;
        self.shuffle = state.shuffle;
        self.repeat = state.repeat;
        self.source = state.source;
        self.source_name = state.source_name;
        self.custom = state.custom;
    }

    pub fn append_track(&mut self, track: Rc<Track>, deduplicate: bool) {
        if !track.available || (deduplicate && self.tracks.iter().any(|t| t.id == track.id)) {
            return;
        }
        if let Some(ref mut original) = self.original_order {
            original.push(track.clone());
        }
        self.tracks.push(track);
        self.custom = true;
    }

    /// Batch form of [`Self::append_track`], with the same rules but one id set
    /// instead of one scan of the queue per track: queueing a 2,000-track
    /// playlist used to be 2,000 × queue-length comparisons on the render thread.
    pub fn append_tracks(&mut self, tracks: Vec<Rc<Track>>, deduplicate: bool) {
        let mut seen: HashSet<i64> = if deduplicate {
            self.tracks.iter().map(|t| t.id).collect()
        } else {
            HashSet::new()
        };
        for track in tracks {
            if !track.available || (deduplicate && !seen.insert(track.id)) {
                continue;
            }
            // Mirror `append_track`, original order included.
            if let Some(original) = self.original_order.as_mut() {
                original.push(track.clone());
            }
            self.tracks.push(track);
            self.custom = true;
        }
    }

    /// Insert tracks so they play immediately after the current one — "play
    /// next", as opposed to [`Self::append_tracks`], which puts them at the end
    /// of the queue.
    ///
    /// A batch keeps its own internal order, so a multi-select plays in the order
    /// it was picked. That means a *second* single insert lands in front of the
    /// first: the most recent "play next" wins, which is what Spotify does and
    /// what makes the action feel immediate.
    ///
    /// With nothing playing there is no "after the current one", so this degrades
    /// to appending. Returns how many tracks were actually inserted, so the
    /// caller can avoid claiming to have queued something the dedupe dropped.
    pub fn insert_next(&mut self, tracks: Vec<Rc<Track>>, deduplicate: bool) -> usize {
        // Note `contains`, not `insert`: the original loop tested against the
        // live queue, which is not modified until the splice below, so a batch
        // may legitimately carry the same track twice. `append_tracks` does
        // dedupe within its batch, because it pushes as it goes — that asymmetry
        // is old behaviour and is kept deliberately.
        let seen: HashSet<i64> = if deduplicate {
            self.tracks.iter().map(|t| t.id).collect()
        } else {
            HashSet::new()
        };
        let accepted: Vec<Rc<Track>> = tracks
            .into_iter()
            .filter(|track| track.available)
            .filter(|track| !deduplicate || !seen.contains(&track.id))
            .collect();
        if accepted.is_empty() {
            return 0;
        }
        // Mirror `append_track`: the shuffled order keeps its own copy, so
        // turning shuffle off later does not lose what was queued.
        if let Some(original) = self.original_order.as_mut() {
            for track in &accepted {
                original.push(track.clone());
            }
        }
        let at = self
            .queue_tail
            // A stale tail (after a manual reorder or removal) must not insert
            // past the end of the list.
            .filter(|tail| *tail <= self.tracks.len())
            .unwrap_or(match self.current_index {
                Some(index) => (index + 1).min(self.tracks.len()),
                None => self.tracks.len(),
            });
        let count = accepted.len();
        self.tracks.splice(at..at, accepted);
        self.queue_tail = Some(at + count);
        self.custom = true;
        count
    }

    pub fn remove_track_at(&mut self, index: usize) -> RemoveOutcome {
        if index >= self.tracks.len() {
            return RemoveOutcome::Unaffected;
        }
        let removed = self.tracks.remove(index);
        self.custom = !self.tracks.is_empty();
        if let Some(ref mut original) = self.original_order
            && let Some(pos) = original.iter().position(|t| t.id == removed.id)
        {
            original.remove(pos);
        }
        match self.current_index {
            Some(cur) if index < cur => {
                self.current_index = Some(cur - 1);
                RemoveOutcome::Unaffected
            }
            Some(cur) if index > cur => RemoveOutcome::Unaffected,
            Some(_) => {
                // index == cur: removed the currently-playing track.
                if index < self.tracks.len() {
                    // The next track shifted into `index`; current_index stays.
                    RemoveOutcome::PlayNext(self.tracks[index].clone())
                } else {
                    self.current_index = None;
                    RemoveOutcome::Stopped
                }
            }
            None => RemoveOutcome::Unaffected,
        }
    }

    /// Move the track at `from` to position `to`, shifting the rest. Keeps the
    /// currently-playing track pointed at the same track. Operates only on the
    /// visible order; the shuffle `original_order` is intentionally left untouched.
    pub fn move_track(&mut self, from: usize, to: usize) {
        let len = self.tracks.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
        self.custom = true;
        if let Some(cur) = self.current_index {
            self.current_index = Some(if cur == from {
                to
            } else if from < to && cur > from && cur <= to {
                cur - 1
            } else if from > to && cur >= to && cur < from {
                cur + 1
            } else {
                cur
            });
        }
    }

    fn apply_shuffle(&mut self) {
        if self.tracks.len() <= 1 {
            self.original_order = Some(self.tracks.clone());
            return;
        }
        self.original_order = Some(self.tracks.clone());

        let current_id = self
            .current_index
            .and_then(|i| self.tracks.get(i))
            .map(|t| t.id);

        let mut rng = rand::rng();
        self.tracks.shuffle(&mut rng);

        if let Some(id) = current_id
            && let Some(pos) = self.tracks.iter().position(|t| t.id == id)
        {
            self.tracks.swap(0, pos);
            self.current_index = Some(0);
        }
    }

    fn restore_original_order(&mut self) {
        let Some(original) = self.original_order.take() else {
            return;
        };
        let current_id = self
            .current_index
            .and_then(|i| self.tracks.get(i))
            .map(|t| t.id);
        self.tracks = original;
        self.current_index = current_id.and_then(|id| self.tracks.iter().position(|t| t.id == id));
    }
}

fn playable(tracks: Vec<Rc<Track>>) -> Vec<Rc<Track>> {
    tracks.into_iter().filter(|track| track.available).collect()
}

/// Rebuild a queue from what was persisted, using what the catalog knows now.
///
/// The id is tried first because it survives a rescan *and* a file moving —
/// `media_items.id` is the durable identity — while a path does not. The path is
/// the fallback for a database that was rebuilt from scratch.
///
/// A ref that matches neither (drive unmounted, file deleted, folder removed)
/// still produces an entry: it keeps its slot, its path and its cue offset, is
/// marked unavailable, and takes its title from the file name. Dropping it
/// instead would silently shrink somebody's queue, and the file is usually only
/// away for a while — the same reason [`crate::playback_queue::PlaybackQueue::restore`]
/// does not filter on availability.
///
/// Pure, so the matching can be tested without a database or a GPUI context.
pub fn resolve_queue_refs(
    refs: &[QueueRef],
    by_id: &HashMap<i64, Track>,
    by_key: &HashMap<(String, i32), Track>,
) -> Vec<Rc<Track>> {
    refs.iter()
        .map(|r| {
            let found = by_id
                .get(&r.id)
                .or_else(|| by_key.get(&(r.path.clone(), r.start_offset_ms)));
            Rc::new(match found {
                Some(track) => track.clone(),
                None => unavailable_from_ref(r),
            })
        })
        .collect()
}

fn unavailable_from_ref(r: &QueueRef) -> Track {
    let title = std::path::Path::new(&r.path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| r.path.clone());
    Track {
        id: r.id,
        title,
        path: r.path.clone(),
        album_id: None,
        track_number: None,
        disc_number: 1,
        duration_ms: None,
        year: None,
        cover_art_id: None,
        start_offset_ms: r.start_offset_ms,
        bitrate: None,
        is_cue: r.start_offset_ms > 0,
        available: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_library::Track;

    fn track(id: i64, path: &str) -> Rc<Track> {
        Rc::new(Track {
            id,
            path: path.to_string(),
            title: format!("Track {}", id),
            album_id: None,
            track_number: Some(id as i32),
            disc_number: 0,
            duration_ms: Some(1000),
            year: None,
            cover_art_id: None,
            start_offset_ms: 0,
            bitrate: None,
            is_cue: false,
            available: true,
        })
    }

    fn missing(id: i64, path: &str) -> Rc<Track> {
        let mut t = track(id, path);
        Rc::make_mut(&mut t).available = false;
        t
    }

    /// An unnamed source must not inherit the previous one's name, or the queue
    /// header would keep saying "From Astroworld" after you queue something else.
    #[test]
    fn the_source_name_clears_when_the_source_is_replaced() {
        let mut q = PlaybackQueue::new();
        q.set_source_named(QueueSource::AllTracks, Some("All tracks".into()));
        assert_eq!(q.source_name().as_deref(), Some("All tracks"));

        q.set_source(QueueSource::Unknown);
        assert_eq!(q.source_name(), None);

        q.set_tracks_with_source(vec![track(1, "/a")], QueueSource::AllTracks);
        assert_eq!(q.source_name(), None);
    }

    #[test]
    fn a_persisted_queue_resolves_by_id_before_path() {
        // Track 1 moved on disk and was re-tagged; track 2 is only reachable by
        // the path it was queued from; track 3 is not in the catalog at all.
        let mut moved = (*track(1, "/old/one.flac")).clone();
        moved.path = "/new/one.flac".to_string();
        moved.title = "Retagged".to_string();
        let by_id = HashMap::from([(1i64, moved)]);
        let by_key = HashMap::from([(
            ("/two.flac".to_string(), 0),
            (*track(2, "/two.flac")).clone(),
        )]);

        let refs = vec![
            QueueRef {
                id: 1,
                path: "/old/one.flac".into(),
                start_offset_ms: 0,
            },
            QueueRef {
                id: 99,
                path: "/two.flac".into(),
                start_offset_ms: 0,
            },
            QueueRef {
                id: 3,
                path: "/gone/missing song.flac".into(),
                start_offset_ms: 0,
            },
        ];

        let out = resolve_queue_refs(&refs, &by_id, &by_key);

        // The id wins: the row comes back with its current path and tags.
        assert_eq!(out[0].title, "Retagged");
        assert_eq!(out[0].path, "/new/one.flac");
        // A stale id falls back to the path the track was queued from.
        assert_eq!(out[1].id, 2);
        // Nothing matched: the entry survives, unavailable, titled by file name.
        assert_eq!(out.len(), 3);
        assert!(!out[2].available);
        assert_eq!(out[2].title, "missing song");
        assert_eq!(out[2].path, "/gone/missing song.flac");
    }

    #[test]
    fn a_cue_piece_keeps_its_offset_when_it_cannot_be_resolved() {
        let refs = vec![QueueRef {
            id: 7,
            path: "/album/image.flac".into(),
            start_offset_ms: 42_000,
        }];

        let out = resolve_queue_refs(&refs, &HashMap::new(), &HashMap::new());

        assert_eq!(out[0].start_offset_ms, 42_000);
        assert!(out[0].is_cue);
    }

    #[test]
    fn append_tracks_dedupes_against_the_queue_and_within_the_batch() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(vec![track(1, "/a"), track(2, "/b")]);

        q.append_tracks(
            vec![track(1, "/a"), track(3, "/c"), track(3, "/c"), missing(4, "/d")],
            true,
        );

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn append_tracks_without_dedupe_keeps_repeats() {
        let mut q = PlaybackQueue::new();
        q.append_tracks(vec![track(1, "/a"), track(1, "/a")], false);

        assert_eq!(q.len(), 2);
        // Still unavailable-filtered: that is not the dedupe's job.
        assert_eq!(q.tracks_vec()[0].id, 1);
    }

    #[test]
    fn play_next_lands_right_after_the_current_track() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(
            vec![track(1, "/a"), track(2, "/b"), track(3, "/c")],
            0,
            QueueSource::Unknown,
        );

        assert_eq!(q.insert_next(vec![track(9, "/z")], false), 1);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 9, 2, 3]);
        assert_eq!(q.next_track().map(|t| t.id), Some(9));
    }

    #[test]
    fn a_second_play_next_lands_behind_the_first() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(vec![track(1, "/a"), track(2, "/b")], 0, QueueSource::Unknown);

        q.insert_next(vec![track(8, "/y")], false);
        q.insert_next(vec![track(9, "/z")], false);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(
            ids,
            vec![1, 8, 9, 2],
            "the first play-next wins and the next one follows it, like Spotify"
        );
    }

    #[test]
    fn separate_queues_play_in_the_order_they_were_queued() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(
            vec![track(1, "/a"), track(2, "/b")],
            0,
            QueueSource::Unknown,
        );

        // Three separate clicks of "play next", as a user would do.
        q.insert_next(vec![track(8, "/x")], false);
        q.insert_next(vec![track(9, "/y")], false);
        q.insert_next(vec![track(10, "/z")], false);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(
            ids,
            vec![1, 8, 9, 10, 2],
            "first queued plays next, and later ones follow it"
        );
        assert_eq!(q.next_track().map(|t| t.id), Some(8));
    }

    #[test]
    fn queueing_again_after_advancing_appends_after_the_pending_ones() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(
            vec![track(1, "/a"), track(2, "/b")],
            0,
            QueueSource::Unknown,
        );
        q.insert_next(vec![track(8, "/x"), track(9, "/y")], false);
        q.next_track();

        // Now playing 8; 9 is still owed. A new queue joins the end of that.
        q.insert_next(vec![track(10, "/z")], false);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 8, 9, 10, 2]);
    }

    #[test]
    fn a_batch_keeps_its_own_order() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(vec![track(1, "/a"), track(2, "/b")], 0, QueueSource::Unknown);

        assert_eq!(q.insert_next(vec![track(8, "/y"), track(9, "/z")], false), 2);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 8, 9, 2]);
    }

    #[test]
    fn play_next_with_nothing_playing_appends() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(vec![track(1, "/a")]);

        q.insert_next(vec![track(9, "/z")], false);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 9], "there is no current track to sit after");
    }

    #[test]
    fn play_next_reports_what_it_actually_queued() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(vec![track(1, "/a"), track(2, "/b")], 0, QueueSource::Unknown);

        assert_eq!(q.insert_next(vec![missing(3, "/c")], false), 0);
        assert_eq!(q.insert_next(vec![track(1, "/a")], true), 0, "already queued");
        assert_eq!(q.insert_next(vec![track(5, "/e")], false), 1);

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 5, 2]);
    }

    #[test]
    fn play_next_keeps_the_shuffled_order_in_sync() {
        let mut q = PlaybackQueue::new();
        q.set_tracks_and_play_at(
            vec![track(1, "/a"), track(2, "/b"), track(3, "/c")],
            0,
            QueueSource::Unknown,
        );
        q.set_shuffle(true);
        let shuffled_len = q.tracks_vec().len();

        q.insert_next(vec![track(9, "/z")], false);

        assert_eq!(q.tracks_vec().len(), shuffled_len + 1);
        let original = q.original_order_vec().expect("shuffle keeps an original order");
        assert!(
            original.iter().any(|t| t.id == 9),
            "the natural order must keep the track too, or unshuffling loses it"
        );
        // And it really is next, whatever the shuffle did to the rest.
        assert_eq!(q.next_track().map(|t| t.id), Some(9));
    }

    #[test]
    fn unavailable_tracks_never_enter_the_queue() {
        let mut q = PlaybackQueue::new();
        let list = vec![track(1, "/a"), missing(2, "/b"), track(3, "/c")];

        let playing = q
            .set_tracks_and_play_at(list.clone(), 2, QueueSource::Unknown)
            .map(|t| t.id);
        assert_eq!(playing, Some(3));
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 3]);

        assert!(
            q.set_tracks_and_play_at(list.clone(), 1, QueueSource::Unknown)
                .is_none()
        );

        q.set_tracks(Vec::new());
        q.append_track(missing(4, "/d"), false);
        assert!(q.is_empty());

        q.set_tracks_and_play_at(vec![track(1, "/a")], 0, QueueSource::Unknown);
        q.refresh_keeping_current(list);
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 3]);
        assert_eq!(q.current_track().map(|t| t.id), Some(1));
    }

    fn track_off(id: i64, path: &str, start_offset_ms: i32) -> Rc<Track> {
        let mut t = track(id, path);
        Rc::make_mut(&mut t).start_offset_ms = start_offset_ms;
        t
    }

    fn fresh_map(tracks: &[Rc<Track>]) -> std::collections::HashMap<(String, i32), Rc<Track>> {
        tracks
            .iter()
            .map(|t| ((t.path.clone(), t.start_offset_ms), t.clone()))
            .collect()
    }

    fn sample_tracks(n: usize) -> Vec<Rc<Track>> {
        (0..n)
            .map(|i| track(i as i64, &format!("/p/{}.flac", i)))
            .collect()
    }

    #[test]
    fn shuffle_on_off_restores_original_order_and_current_track() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(20));
        q.play_track_at(5);

        q.set_shuffle(true);
        assert!(q.shuffle());
        assert_eq!(q.current_track().map(|t| t.id), Some(5));
        // After shuffling, current track is moved to index 0.
        assert_eq!(q.current_index(), Some(0));

        q.set_shuffle(false);
        assert!(!q.shuffle());
        // Original order restored.
        let restored: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(restored, (0..20).collect::<Vec<_>>());
        // Current track preserved by path.
        assert_eq!(q.current_track().map(|t| t.id), Some(5));
        assert_eq!(q.current_index(), Some(5));
    }

    #[test]
    fn repeat_all_wraps_from_last_to_first() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.play_track_at(2);
        q.set_repeat(RepeatMode::All);

        let next = q.next_track().cloned();
        assert_eq!(next.map(|t| t.id), Some(0));
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn repeat_one_returns_same_track() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.play_track_at(1);
        q.set_repeat(RepeatMode::One);

        let next = q.next_track().cloned();
        assert_eq!(next.map(|t| t.id), Some(1));
        assert_eq!(q.current_index(), Some(1));
    }

    #[test]
    fn skipping_past_the_last_track_keeps_it_current() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(2));
        q.play_track_at(1);

        assert!(q.skip_to_next().is_none());
        assert_eq!(q.current_index(), Some(1));
        assert!(matches!(q.previous(0.0), PreviousAction::PreviousTrack(t) if t.id == 0));
    }

    #[test]
    fn repeat_off_stops_at_end() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(2));
        q.play_track_at(1);

        assert!(q.next_track().is_none());
        assert_eq!(q.current_index(), None);
    }

    #[test]
    fn refresh_keeping_current_preserves_shuffle_and_seeds_new_original_order() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(2);
        q.set_shuffle(true);
        assert!(q.shuffle());

        // Pretend the backing playlist gained tracks and we refresh.
        let refreshed: Vec<Rc<Track>> = (10..20)
            .map(|i| track(i, &format!("/r/{}.flac", i)))
            .collect();
        q.refresh_keeping_current(refreshed);

        // Shuffle stays on, original_order is set, set_shuffle(false) now
        // restores into the *new* set's order rather than no-opping.
        assert!(q.shuffle());
        assert!(q.original_order_vec().is_some());
        q.set_shuffle(false);
        let restored: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(restored, (10..20).collect::<Vec<_>>());
    }

    #[test]
    fn remap_assigns_fresh_ids_and_preserves_order_and_current() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(4));
        q.play_track_at(2);
        let refreshed: Vec<Rc<Track>> = (0..4)
            .map(|i| track(100 + i, &format!("/p/{}.flac", i)))
            .collect();
        q.remap_to_fresh_tracks(&fresh_map(&refreshed));
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![100, 101, 102, 103]);
        assert_eq!(q.current_track().map(|t| t.id), Some(102));
        assert_eq!(q.current_index(), Some(2));
    }

    #[test]
    fn remap_drops_non_current_tracks_whose_folder_was_removed() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(4));
        q.play_track_at(2);
        let refreshed = vec![track(102, "/p/2.flac"), track(103, "/p/3.flac")];
        q.remap_to_fresh_tracks(&fresh_map(&refreshed));
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![102, 103]);
        assert_eq!(q.current_track().map(|t| t.id), Some(102));
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn remap_keeps_current_track_when_its_folder_removed() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.play_track_at(1);
        let refreshed = vec![track(100, "/p/0.flac"), track(102, "/p/2.flac")];
        q.remap_to_fresh_tracks(&fresh_map(&refreshed));
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![100, 1, 102]);
        assert_eq!(q.current_track().map(|t| t.id), Some(1));
        assert_eq!(q.current_index(), Some(1));
    }

    #[test]
    fn remap_with_shuffle_remaps_both_orders() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(6));
        q.play_track_at(0);
        q.set_shuffle(true);
        let refreshed: Vec<Rc<Track>> = (0..6)
            .map(|i| track(200 + i, &format!("/p/{}.flac", i)))
            .collect();
        q.remap_to_fresh_tracks(&fresh_map(&refreshed));
        assert!(q.tracks_vec().iter().all(|t| t.id >= 200));
        q.set_shuffle(false);
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, (200..206).collect::<Vec<_>>());
    }

    #[test]
    fn remap_empty_queue_is_noop() {
        let mut q = PlaybackQueue::new();
        q.remap_to_fresh_tracks(&fresh_map(&[]));
        assert!(q.is_empty());
        assert_eq!(q.current_index(), None);
    }

    #[test]
    fn remap_cue_tracks_sharing_path_match_by_offset() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(vec![
            track_off(1, "/album.flac", 0),
            track_off(2, "/album.flac", 5000),
        ]);
        q.play_track_at(1);
        let refreshed = vec![
            track_off(11, "/album.flac", 0),
            track_off(12, "/album.flac", 5000),
        ];
        q.remap_to_fresh_tracks(&fresh_map(&refreshed));
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![11, 12]);
        assert_eq!(q.current_track().map(|t| t.id), Some(12));
    }

    #[test]
    fn set_tracks_while_shuffle_on_reshuffles_and_drops_prior_original() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(10));
        q.play_track_at(0);
        q.set_shuffle(true);

        // New tracks come in (e.g., user picks a new album with shuffle still on).
        let new_tracks: Vec<Rc<Track>> = (100..110)
            .map(|i| track(i, &format!("/np/{}.flac", i)))
            .collect();
        q.set_tracks(new_tracks);

        // Still shuffled — calling set_shuffle(false) must restore the *new* set's order.
        assert!(q.shuffle());
        q.play_track_at(0);
        q.set_shuffle(false);
        let restored: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(restored, (100..110).collect::<Vec<_>>());
    }

    #[test]
    fn set_tracks_and_play_at_with_shuffle_plays_clicked_track() {
        let mut q = PlaybackQueue::new();
        q.set_shuffle(true);
        let tracks = sample_tracks(20);
        let clicked_id = tracks[5].id;
        let result = q
            .set_tracks_and_play_at(tracks, 5, QueueSource::Unknown)
            .cloned();
        assert_eq!(result.map(|t| t.id), Some(clicked_id));
        assert_eq!(q.current_track().map(|t| t.id), Some(clicked_id));
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn set_tracks_and_play_at_without_shuffle_uses_index() {
        let mut q = PlaybackQueue::new();
        let tracks = sample_tracks(10);
        let clicked_id = tracks[3].id;
        let result = q
            .set_tracks_and_play_at(tracks, 3, QueueSource::Unknown)
            .cloned();
        assert_eq!(result.map(|t| t.id), Some(clicked_id));
        assert_eq!(q.current_index(), Some(3));
    }

    #[test]
    fn shuffle_anchors_correct_track_when_paths_collide() {
        // Multi-track files (e.g. CUE sheets) share one path but have distinct ids.
        let tracks = vec![
            track(1, "/album.flac"),
            track(2, "/album.flac"),
            track(3, "/album.flac"),
        ];
        let mut q = PlaybackQueue::new();
        q.set_shuffle(true);
        let clicked = q
            .set_tracks_and_play_at(tracks, 2, QueueSource::Unknown)
            .cloned();
        assert_eq!(clicked.map(|t| t.id), Some(3));
        assert_eq!(q.current_track().map(|t| t.id), Some(3));
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn repeat_mode_cycle_order() {
        assert_eq!(RepeatMode::Off.cycle(), RepeatMode::All);
        assert_eq!(RepeatMode::All.cycle(), RepeatMode::One);
        assert_eq!(RepeatMode::One.cycle(), RepeatMode::Off);
    }

    #[test]
    fn remove_before_current_shifts_index() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(3);
        let outcome = q.remove_track_at(1);
        assert!(matches!(outcome, RemoveOutcome::Unaffected));
        assert_eq!(q.current_index(), Some(2));
        assert_eq!(q.current_track().map(|t| t.id), Some(3));
        assert_eq!(q.len(), 4);
    }

    #[test]
    fn remove_after_current_unchanged() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(2);
        let outcome = q.remove_track_at(4);
        assert!(matches!(outcome, RemoveOutcome::Unaffected));
        assert_eq!(q.current_index(), Some(2));
        assert_eq!(q.current_track().map(|t| t.id), Some(2));
    }

    #[test]
    fn remove_current_non_last_returns_play_next() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(2);
        let outcome = q.remove_track_at(2);
        if let RemoveOutcome::PlayNext(t) = outcome {
            assert_eq!(t.id, 3);
        } else {
            panic!("expected PlayNext");
        }
        assert_eq!(q.current_index(), Some(2));
        assert_eq!(q.current_track().map(|t| t.id), Some(3));
    }

    #[test]
    fn remove_current_last_returns_stopped() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.play_track_at(2);
        let outcome = q.remove_track_at(2);
        assert!(matches!(outcome, RemoveOutcome::Stopped));
        assert_eq!(q.current_index(), None);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn remove_out_of_range_is_noop() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.play_track_at(1);
        let outcome = q.remove_track_at(10);
        assert!(matches!(outcome, RemoveOutcome::Unaffected));
        assert_eq!(q.len(), 3);
        assert_eq!(q.current_index(), Some(1));
    }

    #[test]
    fn remove_with_shuffle_on_cleans_original_order() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(0);
        q.set_shuffle(true);
        // Remove the track at current shuffled index 1 (non-current).
        q.remove_track_at(1);
        assert_eq!(q.len(), 4);
        // original_order should also have one fewer entry.
        let orig = q.original_order_vec().unwrap();
        assert_eq!(orig.len(), 4);
        // Turning shuffle off should restore 4 tracks (not 5).
        q.set_shuffle(false);
        assert_eq!(q.tracks_vec().len(), 4);
    }

    #[test]
    fn move_track_down_shifts_intermediate_current_up() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(2);
        // Drag track 0 down to slot 3; the current (id 2) shifts left by one.
        q.move_track(0, 3);
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 0, 4]);
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(q.current_track().map(|t| t.id), Some(2));
    }

    #[test]
    fn move_track_up_shifts_intermediate_current_down() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(2);
        // Drag track 4 up to slot 1; the current (id 2) shifts right by one.
        q.move_track(4, 1);
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![0, 4, 1, 2, 3]);
        assert_eq!(q.current_index(), Some(3));
        assert_eq!(q.current_track().map(|t| t.id), Some(2));
    }

    #[test]
    fn move_current_track_follows() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(1);
        q.move_track(1, 4);
        assert_eq!(q.current_index(), Some(4));
        assert_eq!(q.current_track().map(|t| t.id), Some(1));
    }

    #[test]
    fn move_track_outside_current_range_leaves_index() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(6));
        q.play_track_at(1);
        // Both endpoints are after the current track — index unaffected.
        q.move_track(3, 5);
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(q.current_track().map(|t| t.id), Some(1));
    }

    #[test]
    fn move_track_noop_cases() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.play_track_at(1);

        q.move_track(1, 1); // same index
        q.move_track(0, 10); // out of range
        q.move_track(10, 0); // out of range

        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
        assert_eq!(q.current_index(), Some(1));
    }

    #[test]
    fn has_next_respects_repeat_mode() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(2));
        q.play_track_at(1);
        assert!(!q.has_next());

        q.set_repeat(RepeatMode::All);
        assert!(q.has_next());

        q.set_repeat(RepeatMode::One);
        assert!(q.has_next());
    }

    #[test]
    fn append_track_marks_queue_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        assert!(!q.is_custom());
        q.append_track(track(10, "/p/10.flac"), false);
        assert!(q.is_custom());
    }

    #[test]
    fn append_track_skipped_by_dedup_keeps_queue_not_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.append_track(track(1, "/p/1.flac"), true);
        assert!(!q.is_custom());
    }

    #[test]
    fn move_track_marks_queue_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.move_track(0, 2);
        assert!(q.is_custom());
    }

    #[test]
    fn move_track_noop_keeps_queue_not_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.move_track(1, 1);
        q.move_track(0, 10);
        assert!(!q.is_custom());
    }

    #[test]
    fn remove_track_marks_queue_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.remove_track_at(1);
        assert!(q.is_custom());
    }

    #[test]
    fn remove_last_remaining_track_clears_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(2));
        q.append_track(track(10, "/p/10.flac"), false);
        q.remove_track_at(2);
        q.remove_track_at(1);
        assert!(q.is_custom());
        q.remove_track_at(0);
        assert!(!q.is_custom());
    }

    #[test]
    fn remove_out_of_range_keeps_queue_not_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.remove_track_at(10);
        assert!(!q.is_custom());
    }

    #[test]
    fn shuffle_does_not_mark_queue_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(5));
        q.play_track_at(0);
        q.set_shuffle(true);
        assert!(!q.is_custom());
        q.set_shuffle(false);
        assert!(!q.is_custom());
    }

    #[test]
    fn replacing_tracks_resets_custom() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.append_track(track(10, "/p/10.flac"), false);
        assert!(q.is_custom());

        q.set_tracks_and_play_at(sample_tracks(4), 0, QueueSource::Unknown);
        assert!(!q.is_custom());

        q.append_track(track(11, "/p/11.flac"), false);
        assert!(q.is_custom());
        q.set_tracks(sample_tracks(2));
        assert!(!q.is_custom());

        q.append_track(track(12, "/p/12.flac"), false);
        q.refresh_keeping_current(sample_tracks(6));
        assert!(!q.is_custom());
    }

    #[test]
    fn restore_round_trips_custom() {
        let mut q = PlaybackQueue::new();
        q.restore(QueueRestore {
            source_name: None,
            tracks: sample_tracks(3),
            original_order: None,
            current_index: Some(1),
            shuffle: false,
            repeat: RepeatMode::Off,
            source: QueueSource::Unknown,
            custom: true,
        });
        assert!(q.is_custom());
    }

    #[test]
    fn append_track_dedup_true_skips_existing() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.append_track(track(1, "/p/1.flac"), true);
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn append_track_dedup_false_allows_duplicate() {
        let mut q = PlaybackQueue::new();
        q.set_tracks(sample_tracks(3));
        q.append_track(track(1, "/p/1.flac"), false);
        assert_eq!(q.len(), 4);
        let ids: Vec<i64> = q.tracks_vec().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![0, 1, 2, 1]);
    }
}
