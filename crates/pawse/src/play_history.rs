//! Listening history.
//!
//! Pulse records what you actually listen to, for its own use: play counts, the
//! recently-played list, and whatever "Rewind" grows into. Nothing in this
//! module touches the network, and there is no target to send anything to —
//! scrobbling and Last.fm were removed in 4.0.
//!
//! Two timings are easy to get wrong and are the reason this is not just a
//! counter:
//!
//! * A listen is timed from the engine's `Playing`/`Paused` events, so time
//!   spent paused is excluded and seeking cannot inflate it. Only wall-clock
//!   time actually spent playing accumulates.
//! * The `plays` table is keyed by `(started_at, artist, title)`. A track that
//!   qualifies mid-listen is written then, and written again at the end with the
//!   real total; both writes must carry the *same* timestamp, or the second one
//!   becomes a duplicate listen instead of an update.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use audio_engine::EngineEvent;
use gpui::{App, BackgroundExecutor, Global, Task};
use music_library::LibraryRepository;
use music_library::models::NewPlay;

use crate::services::Services;

/// The shortest listen worth recording at all.
///
/// Below this a track is a click-through rather than a listen, and recording it
/// would make the play counts mean nothing.
const HISTORY_MIN_SECS: u64 = 15;

/// Whether a listen counts as a *full* one.
///
/// Half the track, or four minutes of something long. These are the thresholds
/// scrobbling used, kept because they are a reasonable definition of "listened
/// to this" — not because anything scrobbles any more. A short track can never
/// qualify, however long it is left on.
pub fn qualifies_as_listen(played: Duration, duration: Duration) -> bool {
    let total = duration.as_secs();
    if total < 30 {
        return false;
    }
    let played = played.as_secs();
    played >= total / 2 || played >= 240
}

/// The artist string to record for a track: every credited artist, joined.
///
/// Local history is only ever read back by the person who created it, so keeping
/// all the artists is strictly more useful than picking one — dropping everyone
/// but the first would misattribute collaborations.
pub fn history_artist(artists: &[String]) -> Option<String> {
    let cleaned: Vec<&str> = artists
        .iter()
        .map(|artist| artist.trim())
        .filter(|artist| !artist.is_empty())
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.join(", "))
}

/// Wall-clock time spent playing, accumulated across pauses.
///
/// Time-based rather than position-based on purpose: a seek must not look like a
/// listen, so jumping to the last ten seconds of a track records ten seconds.
#[derive(Default)]
pub struct PlayAccumulator {
    accumulated: Duration,
    playing_since: Option<Instant>,
}

impl PlayAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_play(&mut self, now: Instant) {
        if self.playing_since.is_none() {
            self.playing_since = Some(now);
        }
    }

    pub fn on_pause(&mut self, now: Instant) {
        if let Some(since) = self.playing_since.take() {
            self.accumulated += now.saturating_duration_since(since);
        }
    }

    pub fn played(&self, now: Instant) -> Duration {
        self.accumulated
            + self
                .playing_since
                .map(|since| now.saturating_duration_since(since))
                .unwrap_or_default()
    }

    pub fn reset(&mut self) {
        self.accumulated = Duration::ZERO;
        self.playing_since = None;
    }
}

/// The fields of a listen that are captured when it starts.
struct CapturedMeta {
    track_id: Option<i64>,
    artist: String,
    title: String,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<u32>,
}

struct Current {
    meta: CapturedMeta,
    duration: Duration,
    /// Stamped once, when playback actually begins. See the module docs.
    timestamp: u64,
    started: bool,
}

struct HistoryState {
    executor: BackgroundExecutor,
    repo: Arc<dyn LibraryRepository>,
    current: Option<Current>,
    accumulator: PlayAccumulator,
}

pub struct PlayHistory {
    state: Rc<RefCell<HistoryState>>,
}

impl Global for PlayHistory {}

/// Start recording listening history for this session.
pub fn setup(cx: &mut App) {
    let repo = cx.global::<Services>().library.repo();
    let state = Rc::new(RefCell::new(HistoryState {
        executor: cx.background_executor().clone(),
        repo,
        current: None,
        accumulator: PlayAccumulator::new(),
    }));
    cx.set_global(PlayHistory {
        state: state.clone(),
    });

    let engine_event_bus = cx.global::<Services>().engine_event_bus.clone();
    cx.subscribe(&engine_event_bus, move |_, event: &EngineEvent, cx| {
        on_engine_event(cx, &state, event);
    })
    .detach();
}

/// Flush the in-flight listen before the app exits, so closing the window
/// mid-track does not silently drop it. The caller waits on the returned task.
pub fn finalize_on_quit(cx: &mut App) -> Option<Task<()>> {
    let state = cx.try_global::<PlayHistory>().map(|s| s.state.clone())?;
    let now = Instant::now();
    let mut st = state.borrow_mut();
    st.accumulator.on_pause(now);
    commit_play_blocking(&mut st, now)
}

fn on_engine_event(cx: &App, state: &Rc<RefCell<HistoryState>>, event: &EngineEvent) {
    let now = Instant::now();
    match event {
        EngineEvent::Loaded { duration, .. } => {
            let meta = read_current_meta(cx);
            let is_playing = cx.global::<Services>().is_playing.load(Ordering::Relaxed);
            let mut st = state.borrow_mut();
            st.accumulator.on_pause(now);
            commit_play(&mut st, now, Commit::Final);
            st.current = None;
            st.accumulator.reset();
            if let Some(meta) = meta.filter(|m| !m.artist.is_empty() && !m.title.is_empty()) {
                st.current = Some(Current {
                    meta,
                    duration: *duration,
                    timestamp: 0,
                    started: false,
                });
                if is_playing {
                    st.accumulator.on_play(now);
                    mark_started(&mut st);
                }
            }
        }
        EngineEvent::Playing => {
            let mut st = state.borrow_mut();
            st.accumulator.on_play(now);
            mark_started(&mut st);
        }
        EngineEvent::Paused => {
            let mut st = state.borrow_mut();
            st.accumulator.on_pause(now);
            commit_if_qualified(&mut st, now);
        }
        EngineEvent::TrackEnded | EngineEvent::Stopped => {
            let mut st = state.borrow_mut();
            st.accumulator.on_pause(now);
            commit_play(&mut st, now, Commit::Final);
            st.current = None;
            st.accumulator.reset();
        }
        _ => {}
    }
}

/// Note that playback has begun, and stamp the listen exactly once.
fn mark_started(st: &mut HistoryState) {
    let Some(current) = st.current.as_mut() else {
        return;
    };
    if current.started {
        return;
    }
    current.started = true;
    current.timestamp = unix_now();
}

/// On pause, write a qualified listen early rather than waiting for the end.
///
/// A track paused and then abandoned would otherwise never be counted.
fn commit_if_qualified(st: &mut HistoryState, now: Instant) {
    let played = st.accumulator.played(now);
    let duration = st
        .current
        .as_ref()
        .map(|current| current.duration)
        .unwrap_or_default();
    if !qualifies_as_listen(played, duration) {
        return;
    }
    commit_play(st, now, Commit::Live);
}

#[derive(Clone, Copy)]
enum Commit {
    /// Written mid-listen, when it qualified.
    Live,
    /// Written when the track is finished, skipped, or replaced.
    Final,
}

fn pending_play(current: &Current, played: Duration, commit: Commit) -> Option<NewPlay> {
    if !current.started || current.meta.artist.is_empty() || current.meta.title.is_empty() {
        return None;
    }
    let qualified = qualifies_as_listen(played, current.duration);
    // A live commit only exists to record a qualified listen; a final commit
    // also records real but partial listens, as long as they are not so short
    // that they were never really a listen.
    if !qualified && (matches!(commit, Commit::Live) || played.as_secs() < HISTORY_MIN_SECS) {
        return None;
    }
    Some(NewPlay {
        track_id: current.meta.track_id,
        artist: current.meta.artist.clone(),
        title: current.meta.title.clone(),
        album: current.meta.album.clone(),
        album_artist: current.meta.album_artist.clone(),
        track_number: current.meta.track_number,
        duration_secs: Some(current.duration.as_secs()),
        played_secs: Some(played.as_secs()),
        started_at: current.timestamp,
        qualified,
    })
}

fn commit_play(st: &mut HistoryState, now: Instant, commit: Commit) {
    let played = st.accumulator.played(now);
    let Some(play) = st
        .current
        .as_ref()
        .and_then(|current| pending_play(current, played, commit))
    else {
        return;
    };
    let repo = st.repo.clone();
    st.executor
        .spawn(async move {
            if let Err(err) = repo.record_play(&play) {
                log::warn!("history: could not record a play: {err}");
            }
        })
        .detach();
}

fn commit_play_blocking(st: &mut HistoryState, now: Instant) -> Option<Task<()>> {
    let played = st.accumulator.played(now);
    let play = st
        .current
        .as_ref()
        .and_then(|current| pending_play(current, played, Commit::Final))?;
    let repo = st.repo.clone();
    Some(st.executor.spawn(async move {
        if let Err(err) = repo.record_play(&play) {
            log::warn!("history: could not record the closing play: {err}");
        }
    }))
}

fn read_current_meta(cx: &App) -> Option<CapturedMeta> {
    let services = cx.global::<Services>();
    let queue = services.playback_queue.borrow();
    let track = queue.current_track()?;
    Some(captured_meta(&*services.library.repo(), track))
}

fn captured_meta(repo: &dyn LibraryRepository, track: &music_library::Track) -> CapturedMeta {
    let artist = history_artist(&repo.track_artists(track.id).unwrap_or_default())
        .unwrap_or_default();
    let album = track
        .album_id
        .and_then(|id| repo.album_title(id).ok().flatten());
    let album_artist = track
        .album_id
        .and_then(|id| history_artist(&repo.album_artists(id).unwrap_or_default()));
    CapturedMeta {
        track_id: Some(track.id),
        artist,
        title: track.title.clone(),
        album,
        album_artist,
        track_number: track.track_number.and_then(|n| u32::try_from(n).ok()),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_current(duration_secs: u64) -> Current {
        Current {
            meta: CapturedMeta {
                track_id: Some(7),
                artist: "Tool".into(),
                title: "Pneuma".into(),
                album: None,
                album_artist: None,
                track_number: None,
            },
            duration: Duration::from_secs(duration_secs),
            timestamp: 1_700_000_000,
            started: true,
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn pausing_before_the_threshold_commits_nothing() {
        let current = a_current(300);
        assert!(
            pending_play(&current, secs(30), Commit::Live).is_none(),
            "a 30s pause in a 300s track must not consume the play"
        );
    }

    #[test]
    fn the_same_track_finished_later_still_qualifies() {
        let current = a_current(300);
        let play = pending_play(&current, secs(300), Commit::Final).unwrap();
        assert!(play.qualified);
        assert_eq!(play.played_secs, Some(300));
    }

    #[test]
    fn a_qualified_pause_commits_early_and_the_end_reuses_the_same_timestamp() {
        let current = a_current(300);
        let early = pending_play(&current, secs(200), Commit::Live).unwrap();
        assert!(early.qualified);
        assert_eq!(early.played_secs, Some(200));

        let total = pending_play(&current, secs(300), Commit::Final).unwrap();
        assert_eq!(total.played_secs, Some(300));
        assert_eq!(
            total.started_at, early.started_at,
            "both commits must land on the same row, or the second becomes a second play"
        );
    }

    #[test]
    fn a_click_through_is_never_recorded() {
        let current = a_current(300);
        assert!(pending_play(&current, secs(14), Commit::Final).is_none());
        assert!(pending_play(&current, secs(15), Commit::Final).is_some());
    }

    #[test]
    fn a_skip_past_the_floor_is_history_without_qualifying() {
        let current = a_current(600);
        let play = pending_play(&current, secs(25), Commit::Final).unwrap();
        assert!(!play.qualified);
        assert_eq!(play.played_secs, Some(25));
    }

    #[test]
    fn a_track_that_never_started_is_ignored() {
        let mut current = a_current(300);
        current.started = false;
        assert!(pending_play(&current, secs(300), Commit::Final).is_none());
    }

    #[test]
    fn a_track_under_thirty_seconds_never_qualifies() {
        let current = a_current(29);
        let play = pending_play(&current, secs(29), Commit::Final).unwrap();
        assert!(!play.qualified);
    }

    #[test]
    fn a_short_track_never_qualifies() {
        assert!(!qualifies_as_listen(secs(29), secs(29)));
        assert!(!qualifies_as_listen(secs(1000), secs(20)));
    }

    #[test]
    fn half_listened_qualifies() {
        assert!(qualifies_as_listen(secs(60), secs(120)));
        assert!(!qualifies_as_listen(secs(59), secs(120)));
    }

    #[test]
    fn four_minutes_qualifies_a_long_track() {
        assert!(qualifies_as_listen(secs(240), secs(600)));
        assert!(!qualifies_as_listen(secs(239), secs(600)));
    }

    #[test]
    fn history_keeps_every_artist() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            history_artist(&names(&["A", "B"])).as_deref(),
            Some("A, B"),
            "a collaboration must not be reduced to its first name"
        );
        assert_eq!(
            history_artist(&names(&["  Boards of Canada  "])).as_deref(),
            Some("Boards of Canada")
        );
        assert_eq!(history_artist(&names(&["", "  "])), None);
        assert_eq!(history_artist(&[]), None);
    }

    #[test]
    fn seeking_does_not_inflate_played_time() {
        let t0 = Instant::now();
        let mut acc = PlayAccumulator::new();
        acc.on_play(t0);
        acc.on_pause(t0 + secs(30));
        acc.on_play(t0 + secs(40));
        assert_eq!(acc.played(t0 + secs(50)), secs(40));
    }

    #[test]
    fn paused_time_is_excluded() {
        let t0 = Instant::now();
        let mut acc = PlayAccumulator::new();
        acc.on_play(t0);
        acc.on_pause(t0 + secs(30));
        assert_eq!(acc.played(t0 + secs(100)), secs(30));
    }

    #[test]
    fn played_counts_an_ongoing_span() {
        let t0 = Instant::now();
        let mut acc = PlayAccumulator::new();
        acc.on_play(t0);
        assert_eq!(acc.played(t0 + secs(75)), secs(75));
    }

    #[test]
    fn reset_starts_a_fresh_track() {
        let t0 = Instant::now();
        let mut acc = PlayAccumulator::new();
        acc.on_play(t0);
        acc.on_pause(t0 + secs(30));
        acc.reset();
        assert_eq!(acc.played(t0 + secs(100)), Duration::ZERO);
    }
}
