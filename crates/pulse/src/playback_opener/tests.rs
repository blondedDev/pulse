//! Local-only opener tests.
//!
//! These replaced a much larger suite that exercised the streaming path
//! (`RemoteMedia`, abort gates, byte-range reads). With media servers gone,
//! every track is a file on disk, so what is worth pinning down is: the fast
//! path writes nothing to the database, the slow path still finds a candidate
//! that moved, and a superseded resolve never lands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use audio_engine::Command;

use super::*;

/// What the opener told the engine, in a form that is cheap to assert on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sent {
    Local {
        path: PathBuf,
        offset: Option<Duration>,
        prepared: bool,
    },
    Prepare {
        play: Option<bool>,
    },
    Play {
        fade_in: bool,
    },
    Fail(String),
    Stop,
    Other,
}

fn record(commands: &Arc<Mutex<Vec<Sent>>>) -> Sink {
    let commands = commands.clone();
    Arc::new(move |command: Command| {
        let sent = match command {
            Command::SetLocalTrack {
                path,
                start_offset,
                prepared,
                ..
            } => Sent::Local {
                path,
                offset: start_offset,
                prepared,
            },
            Command::Prepare { play, .. } => Sent::Prepare { play },
            Command::Play { fade_in } => Sent::Play { fade_in },
            Command::Fail(message) => Sent::Fail(message),
            Command::Stop => Sent::Stop,
            _ => Sent::Other,
        };
        commands.lock().unwrap().push(sent);
    })
}

/// A backend with the candidate list spelled out per track id.
type CandidateEntries = Vec<(i64, Vec<(String, i64)>)>;

struct Candidates(Mutex<CandidateEntries>);

impl Candidates {
    fn new(entries: CandidateEntries) -> Arc<Self> {
        Arc::new(Self(Mutex::new(entries)))
    }
}

impl OpenerBackend for Candidates {
    fn locators(&self, track_id: i64) -> Vec<(String, i64)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == track_id)
            .map(|(_, locators)| locators.clone())
            .unwrap_or_default()
    }
}

fn temp_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pulse-opener-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, b"audio").unwrap();
    path
}

fn request(path: &Path, id: i64) -> TrackRequest {
    TrackRequest {
        id,
        path: path.to_string_lossy().into_owned(),
        start_offset_ms: 0,
        duration: Some(Duration::from_secs(3)),
    }
}

/// The worker thread talks to the sink asynchronously; give it a moment.
fn drain(commands: &Arc<Mutex<Vec<Sent>>>) -> Vec<Sent> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let seen = {
            let guard = commands.lock().unwrap();
            if guard.iter().any(|sent| matches!(sent, Sent::Local { .. })) {
                return guard.clone();
            }
            guard.clone()
        };
        if std::time::Instant::now() > deadline {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_existing_file_is_handed_straight_to_the_engine() {
    let path = temp_file("direct.wav");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let backend = Candidates::new(Vec::new());
    let opener = PlaybackOpener::new(backend, record(&commands));

    opener.start(&request(&path, 1), AfterLoad::Play);

    let sent = commands.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![
            Sent::Local {
                path: path.clone(),
                offset: None,
                prepared: false,
            },
            Sent::Play { fade_in: true },
        ],
        "a present file must not go through Prepare or a worker thread"
    );
}

#[test]
fn a_stay_load_does_not_autoplay() {
    let path = temp_file("stay.wav");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let backend = Candidates::new(Vec::new());
    let opener = PlaybackOpener::new(backend, record(&commands));

    opener.start(&request(&path, 1), AfterLoad::Stay);

    assert_eq!(
        commands.lock().unwrap().clone(),
        vec![Sent::Local {
            path,
            offset: None,
            prepared: false,
        }]
    );
}

#[test]
fn a_missing_file_falls_back_to_a_candidate_that_exists() {
    let missing = PathBuf::from("/definitely/not/here.wav");
    let present = temp_file("moved.wav");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let backend = Candidates::new(vec![(
        7,
        vec![
            ("/also/missing.wav".to_string(), 0),
            (present.to_string_lossy().into_owned(), 1500),
        ],
    )]);
    let opener = PlaybackOpener::new(backend, record(&commands));

    opener.start(&request(&missing, 7), AfterLoad::Play);

    let sent = drain(&commands);
    assert_eq!(
        sent.first(),
        Some(&Sent::Prepare { play: Some(true) }),
        "the slow path announces itself first"
    );
    assert_eq!(
        sent.last(),
        Some(&Sent::Local {
            path: present,
            offset: Some(Duration::from_millis(1500)),
            prepared: true,
        }),
        "the first candidate that exists wins, with its own offset"
    );
}

#[test]
fn when_nothing_exists_the_track_path_is_still_tried_so_the_engine_reports_it() {
    let missing = PathBuf::from("/definitely/not/here.wav");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let backend = Candidates::new(vec![(
        9,
        vec![("/also/missing.wav".to_string(), 0)],
    )]);
    let opener = PlaybackOpener::new(backend, record(&commands));

    opener.start(&request(&missing, 9), AfterLoad::Play);

    let sent = drain(&commands);
    assert_eq!(
        sent.last(),
        Some(&Sent::Local {
            path: missing,
            offset: None,
            prepared: true,
        })
    );
}

#[test]
fn a_superseded_resolve_never_reaches_the_engine() {
    let missing = PathBuf::from("/definitely/not/here.wav");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let backend = Candidates::new(vec![(
        11,
        vec![("/also/missing.wav".to_string(), 0)],
    )]);
    let opener = PlaybackOpener::new(backend, record(&commands));

    opener.start(&request(&missing, 11), AfterLoad::Play);
    // Bump the generation before the worker can report back.
    opener.stop();

    // Give the worker time to finish and (wrongly) emit, if it were going to.
    std::thread::sleep(Duration::from_millis(300));
    let sent = commands.lock().unwrap().clone();
    assert!(
        !sent.iter().any(|s| matches!(s, Sent::Local { prepared: true, .. })),
        "a stale resolve must be dropped, got {sent:?}"
    );
    assert_eq!(sent.last(), Some(&Sent::Stop));
}
