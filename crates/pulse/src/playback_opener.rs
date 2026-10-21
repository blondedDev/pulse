use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use audio_engine::Command;
use music_library::Track;

use crate::library_service::LibraryService;

pub type Sink = Arc<dyn Fn(Command) + Send + Sync>;

/// Where the opener finds the audio it hands to the engine.
///
/// Every track resolves to a file on disk. The trait exists so tests can supply
/// locators without a library, and so a future source can slot in without
/// touching `PlaybackOpener`.
pub trait OpenerBackend: Send + Sync {
    /// Candidate paths for a track id, in preference order. Each carries the
    /// offset to start at (cue pieces of a single file share it). An empty
    /// result means "just use the track's own path".
    fn locators(&self, track_id: i64) -> Vec<(String, i64)>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterLoad {
    Stay,
    Play,
    PlayGapless,
}

impl AfterLoad {
    fn autoplay(self) -> Option<bool> {
        match self {
            AfterLoad::Stay => None,
            AfterLoad::Play => Some(true),
            AfterLoad::PlayGapless => Some(false),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackRequest {
    pub id: i64,
    pub path: String,
    pub start_offset_ms: i64,
    pub duration: Option<Duration>,
}

impl From<&Track> for TrackRequest {
    fn from(track: &Track) -> Self {
        Self {
            id: track.id,
            path: track.path.clone(),
            start_offset_ms: i64::from(track.start_offset_ms),
            duration: track
                .duration_ms
                .map(|ms| Duration::from_millis(ms.max(0) as u64)),
        }
    }
}

#[derive(Clone)]
pub struct PlaybackOpener {
    backend: Arc<dyn OpenerBackend>,
    sink: Sink,
    generation: Arc<AtomicU64>,
}

fn offset(start_ms: i64) -> Option<Duration> {
    (start_ms > 0).then(|| Duration::from_millis(start_ms as u64))
}

impl PlaybackOpener {
    pub fn new(backend: Arc<dyn OpenerBackend>, sink: Sink) -> Self {
        Self {
            backend,
            sink,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn stop(&self) {
        self.supersede();
        (self.sink)(Command::Stop);
    }

    pub fn start(&self, track: &TrackRequest, after: AfterLoad) {
        let generation = self.supersede();

        // The overwhelmingly common case: the file is where the library says it
        // is. Hand it straight to the engine — no worker thread, no database
        // hit, so gapless transitions stay tight.
        let path = PathBuf::from(&track.path);
        if path.exists() {
            (self.sink)(Command::SetLocalTrack {
                path,
                start_offset: offset(track.start_offset_ms),
                track_duration: track.duration,
                prepared: false,
            });
            if let Some(fade_in) = after.autoplay() {
                (self.sink)(Command::Play { fade_in });
            }
            return;
        }

        // Otherwise the path is stale or the track is one piece of a shared
        // file: resolving needs the database, so do it off the main thread.
        (self.sink)(Command::Prepare {
            play: after.autoplay(),
            track_duration: track.duration,
        });
        let opener = self.clone();
        let track = track.clone();
        let spawned = std::thread::Builder::new()
            .name("track-opener".into())
            .spawn(move || opener.open(&track, generation));
        if let Err(e) = spawned {
            (self.sink)(Command::Fail(e.to_string()));
        }
    }

    /// Invalidate any in-flight resolution. The returning worker notices via
    /// `is_stale` and drops its command instead of clobbering the new track.
    fn supersede(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn is_stale(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) != generation
    }

    fn open(&self, track: &TrackRequest, generation: u64) {
        let Some(command) = self.resolve(track, generation) else {
            return;
        };
        if !self.is_stale(generation) {
            (self.sink)(command);
        }
    }

    /// First candidate that exists on disk wins. If none do, hand the track's
    /// own path to the engine anyway so it reports the failure — that keeps the
    /// "file is gone" message identical to a direct load.
    fn resolve(&self, track: &TrackRequest, generation: u64) -> Option<Command> {
        let mut candidates = self.backend.locators(track.id);
        if candidates.is_empty() {
            candidates.push((track.path.clone(), track.start_offset_ms));
        }
        for (locator, start_ms) in candidates {
            if self.is_stale(generation) {
                return None;
            }
            let candidate = PathBuf::from(&locator);
            if candidate.exists() {
                return Some(self.local(candidate, start_ms, track.duration));
            }
        }
        Some(self.local(
            PathBuf::from(&track.path),
            track.start_offset_ms,
            track.duration,
        ))
    }

    fn local(&self, path: PathBuf, start_ms: i64, duration: Option<Duration>) -> Command {
        Command::SetLocalTrack {
            path,
            start_offset: offset(start_ms),
            track_duration: duration,
            prepared: true,
        }
    }
}

pub struct LibraryBackend {
    pub library: Arc<LibraryService>,
}

impl OpenerBackend for LibraryBackend {
    fn locators(&self, track_id: i64) -> Vec<(String, i64)> {
        self.library.playback_locators(track_id)
    }
}

#[cfg(test)]
mod tests;
