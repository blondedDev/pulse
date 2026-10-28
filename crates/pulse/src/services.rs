use std::{
    cell::RefCell,
    collections::HashMap,
    io::Read,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::playback_opener::AfterLoad;
use audio_engine::{AudioEngine, EngineEvent, EngineManager};
use audio_output::{AudioOutput, Output};
use gpui::{App, AppContext, AsyncApp, Entity, EventEmitter, Global, SharedString};
use gpui_component::WindowExt;
use gpui_component::notification::Notification;
use music_library::Track;

use crate::cover_art_cache::CoverArtCache;
use crate::library_service::{LibraryEvent, LibraryService};

#[derive(Clone)]
pub struct Services {
    pub engine_manager: Rc<EngineManager>,
    pub output: Arc<Output>,
    pub engine_event_bus: Entity<EngineEventsBus>,
    pub playback_status: Entity<crate::playback_status::PlaybackStatus>,
    pub library: Arc<LibraryService>,
    opener: crate::playback_opener::PlaybackOpener,
    pub is_buffering: Arc<AtomicBool>,
    pub resume_at: Rc<std::cell::Cell<Option<(i64, u64)>>>,
    pub library_event_bus: Entity<LibraryEventsBus>,
    pub playback_queue: Rc<RefCell<crate::playback_queue::PlaybackQueue>>,
    /// The track row the pointer is currently over, if any.
    ///
    /// The `Q` shortcut acts on "the row under the cursor", which no gpui element
    /// can hand you: hover exists only as a style group, not as state. Rows write
    /// here on hover and clear it on leave. A single window and a single pointer
    /// make one slot enough.
    pub hovered_track: Rc<RefCell<Option<Rc<music_library::Track>>>>,
    pub cover_art_cache: Rc<RefCell<CoverArtCache>>,
    pub current_position_ms: Arc<AtomicU64>,
    pub current_duration_ms: Arc<AtomicU64>,
    /// Last-loaded track's DSD rate, cached outside the one-shot
    /// `EngineEvent::Loaded` event so it survives window teardown/rebuild
    /// (e.g. macOS close + dock-icon reopen, which recreates `NowPlaying`
    /// without reloading the track). 0 means "not DSD" — `dsd_rate` is
    /// never legitimately zero (`dsd::DsdSource::open` itself rejects it).
    pub current_dsd_rate: Arc<AtomicU32>,
    pub is_playing: Arc<AtomicBool>,
    pub playlist_popup_bus: Entity<crate::playlist_popup::PlaylistPopupBus>,
    pub lang_event_bus: Entity<crate::localization::LangEventBus>,
    pub library_watcher: Rc<RefCell<Option<crate::library_watcher::LibraryWatcher>>>,
    pub watcher_ping_tx: flume::Sender<()>,
    pub remote_handle: pulse_remote::StateHandle,
    pub remote_state_rx: pulse_remote::StateRx,
    pub remote_command_tx: pulse_remote::CommandSink,
    pub remote_server: Rc<RefCell<Option<pulse_remote::RemoteServer>>>,
    remote_queue_cache: Rc<RefCell<RemoteQueueCache>>,
    library_rev: Arc<AtomicU64>,
}

type RemoteQueueCache = Option<(u64, Arc<Vec<pulse_remote::QueueItem>>)>;

impl Services {
    pub fn initialize(cx: &mut App) -> anyhow::Result<Self> {
        let output = Arc::new(Output::new());
        let audio_engine = Rc::new(AudioEngine::new(output.clone()));
        let engine_manager = Rc::new(EngineManager::new(audio_engine).start(cx));
        let engine_event_bus = cx.new(|_| EngineEventsBus);
        let playback_status = crate::playback_status::PlaybackStatus::create(&engine_event_bus, cx);

        let (library_event_tx, library_event_rx) = flume::unbounded();
        let library = Arc::new(LibraryService::new(
            library_event_tx,
            cx.background_executor().clone(),
            cx.global::<crate::settings_store::SettingsStore>()
                .artists_grouping(),
        )?);
        let library_event_bus = cx.new(|_| LibraryEventsBus);
        let library_event_bus_clone = library_event_bus.clone();

        cx.spawn(async move |cx| {
            while let Ok(event) = library_event_rx.recv_async().await {
                cx.update(|cx| {
                    // If a playlist that's currently backing the playback
                    // queue has its track list changed, sync the queue first
                    // so subscribers (queue_view, etc.) see fresh state when
                    // they receive the emitted event below.
                    if let LibraryEvent::PlaylistTracksChanged { playlist_id } = &event {
                        sync_queue_with_playlist(*playlist_id, cx);
                    }
                    if matches!(&event, LibraryEvent::CatalogChanged) {
                        remap_queue_after_rescan(cx);
                    }
                    if matches!(&event, LibraryEvent::TagsSaved) {
                        let cache = cx.global::<Services>().cover_art_cache.clone();
                        cache.borrow_mut().clear(cx);
                    }
                    let library_changed = matches!(
                        &event,
                        LibraryEvent::PlaylistsChanged
                            | LibraryEvent::PlaylistTracksChanged { .. }
                            | LibraryEvent::CatalogChanged
                    );
                    if library_changed {
                        cx.global::<Services>()
                            .library_rev
                            .fetch_add(1, Ordering::Relaxed);
                        publish_remote_state(cx);
                    }
                    notify_scan_event(&event, cx);
                    library_event_bus_clone.update(cx, |_, cx| cx.emit(event));
                });
            }
        })
        .detach();

        let (watcher_ping_tx, watcher_ping_rx) = flume::bounded::<()>(1);
        let watcher_library = library.clone();
        cx.spawn(async move |cx| {
            while watcher_ping_rx.recv_async().await.is_ok() {
                cx.update(|cx| {
                    let folders = cx
                        .global::<crate::settings_store::SettingsStore>()
                        .music_folders()
                        .to_vec();
                    watcher_library.watched_change(folders);
                });
            }
        })
        .detach();

        let commander = engine_manager.commander();
        let opener = crate::playback_opener::PlaybackOpener::new(
            Arc::new(crate::playback_opener::LibraryBackend {
                library: library.clone(),
            }),
            Arc::new(move |command| commander.send(command)),
        );

        let playlist_popup_bus = cx.new(|_| crate::playlist_popup::PlaylistPopupBus);
        let lang_event_bus = cx.new(|_| crate::localization::LangEventBus);

        let (remote_handle, remote_state_rx) = pulse_remote::channel();
        let (remote_command_tx, remote_command_rx) = pulse_remote::commands();

        cx.spawn(async move |cx| {
            while let Ok(command) = remote_command_rx.recv_async().await {
                cx.update(|cx| apply_remote_command(cx, command));
            }
        })
        .detach();

        Ok(Services {
            output,
            engine_manager,
            engine_event_bus,
            playback_status,
            library,
            library_event_bus,
            playback_queue: Rc::new(RefCell::new(crate::playback_queue::PlaybackQueue::new())),
            hovered_track: Rc::new(RefCell::new(None)),
            cover_art_cache: Rc::new(RefCell::new(CoverArtCache::new())),
            current_position_ms: Arc::new(AtomicU64::new(0)),
            current_duration_ms: Arc::new(AtomicU64::new(0)),
            current_dsd_rate: Arc::new(AtomicU32::new(0)),
            is_playing: Arc::new(AtomicBool::new(false)),
            playlist_popup_bus,
            lang_event_bus,
            library_watcher: Rc::new(RefCell::new(None)),
            watcher_ping_tx,
            remote_handle,
            remote_state_rx,
            remote_command_tx,
            remote_server: Rc::new(RefCell::new(None)),
            remote_queue_cache: Rc::new(RefCell::new(None)),
            library_rev: Arc::new(AtomicU64::new(0)),
            opener,
            is_buffering: Arc::new(AtomicBool::new(false)),
            resume_at: Rc::new(std::cell::Cell::new(None)),
        })
    }

    pub fn play_track(&self, track: &Track, cx: &App) {
        self.start_track(track, AfterLoad::Play, cx);
    }

    /// Like `play_track` but skips the 300ms fade-in for gapless transitions.
    pub fn play_track_gapless(&self, track: &Track, cx: &App) {
        self.start_track(track, AfterLoad::PlayGapless, cx);
    }

    /// Load a track into the engine without starting playback. Fires
    /// `EngineEvent::Loaded` so subscribers (now-playing, queue view) update,
    /// but leaves the engine paused at position 0.
    pub fn load_track(&self, track: &Track, cx: &App) {
        self.start_track(track, AfterLoad::Stay, cx);
    }

    pub fn stop_playback(&self) {
        self.opener.stop();
    }

    fn start_track(&self, track: &Track, after: AfterLoad, cx: &App) {
        self.current_position_ms.store(0, Ordering::Relaxed);
        self.engine_manager
            .set_track_gain(track_gain_for(self, track, cx));
        self.opener.start(&track.into(), after);
    }
}

impl Services {
    pub fn shutdown(&self) {
        self.output.shutdown();
        self.engine_manager.shutdown();
    }

    pub fn snapshot_playback(&self) -> crate::settings_store::PlaybackState {
        let queue = self.playback_queue.borrow();
        // Only a locator per entry is persisted — the catalog holds the rest and
        // is re-read when the queue is restored, so a tag edit shows up in the
        // queue after a restart instead of a title frozen at queue time.
        crate::settings_store::PlaybackState {
            queue: queue.tracks_vec().iter().map(queue_ref).collect(),
            original_queue: queue
                .original_order_vec()
                .map(|v| v.iter().map(queue_ref).collect()),
            current_index: queue.current_index(),
            position_ms: self.current_position_ms.load(Ordering::Relaxed),
            shuffle: queue.shuffle(),
            repeat: queue.repeat().into(),
            source: queue.source().into(),
            source_name: queue.source_name().map(|name| name.to_string()),
            custom: queue.is_custom(),
        }
    }
}

/// The linear ReplayGain multiplier for `track`, or 1.0 when it is off or the
/// file carries no tags.
///
/// One indexed database read per track load — far cheaper than carrying four
/// floats on every `Track` a list ever renders.
pub(crate) fn track_gain_for(services: &Services, track: &Track, cx: &App) -> f32 {
    use crate::settings_store::ReplayGainMode;
    let store = cx.global::<crate::settings_store::SettingsStore>();
    let mode = store.replaygain();
    if mode == ReplayGainMode::Off {
        return 1.0;
    }
    let Some(gain) = services.library.replaygain(track.id) else {
        return 1.0;
    };
    gain.linear(
        mode == ReplayGainMode::Album,
        store.replaygain_prevent_clipping(),
    )
}

/// Re-apply the ReplayGain level to the track that is already playing.
///
/// A mode change is audible immediately rather than at the next track; the
/// engine stores the multiplier, so nothing else about playback moves.
pub fn refresh_track_gain(cx: &mut App) {
    let services = cx.global::<Services>();
    let Some(current) = services.playback_queue.borrow().current_track().cloned() else {
        return;
    };
    services
        .engine_manager
        .set_track_gain(track_gain_for(services, &current, cx));
}

fn queue_ref(track: &Rc<Track>) -> crate::settings_store::QueueRef {
    crate::settings_store::QueueRef {
        id: track.id,
        path: track.path.clone(),
        start_offset_ms: track.start_offset_ms,
    }
}

/// Rebuild the saved queue into tracks by asking the catalog what it still has.
///
/// Takes the repository rather than `LibraryService` so this stays testable
/// without a GPUI context — the queries are the part worth testing, not the
/// wiring around them. Two batched lookups, not one per entry: a restored queue
/// can be thousands of tracks. See
/// [`crate::playback_queue::resolve_queue_refs`] for what happens to an entry the
/// catalog has lost.
pub fn restore_queue(
    refs: &[crate::settings_store::QueueRef],
    repo: &dyn music_library::LibraryRepository,
) -> Vec<Rc<Track>> {
    if refs.is_empty() {
        return Vec::new();
    }

    let by_id: HashMap<i64, Track> = repo
        .tracks_by_ids(&refs.iter().map(|r| r.id).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|track| (track.id, track))
        .collect();

    // Only ask for paths the id lookup could not answer. A queue restored from a
    // database that is still intact resolves entirely through the first query.
    let unresolved: Vec<(String, i32)> = refs
        .iter()
        .filter(|r| !by_id.contains_key(&r.id))
        .map(|r| (r.path.clone(), r.start_offset_ms))
        .collect();
    let by_key: HashMap<(String, i32), Track> = if unresolved.is_empty() {
        HashMap::new()
    } else {
        repo.tracks_by_keys(&unresolved)
            .unwrap_or_default()
            .into_iter()
            .map(|track| ((track.path.clone(), track.start_offset_ms), track))
            .collect()
    };

    crate::playback_queue::resolve_queue_refs(refs, &by_id, &by_key)
}

fn notify_scan_event(event: &LibraryEvent, cx: &mut App) {
    let notification = match event {
        LibraryEvent::ScanFailed => {
            Notification::error(crate::localization::tr().library_update_failed.clone())
        }
        LibraryEvent::ScanFolderUnavailable { folder } => {
            Notification::warning(crate::localization::tr().library_folder_unavailable(folder))
        }
        LibraryEvent::TagsSaved => {
            Notification::success(crate::localization::tr().tags_saved.clone())
        }
        _ => return,
    };
    let Some(handle) = cx.windows().into_iter().next() else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.push_notification(notification, cx);
    });
}

/// If the active playback queue is backed by the given playlist, replace its
/// track list with a fresh copy from the library, preserving the currently-
/// playing track by id. Also snapshots the new queue into the settings store
/// so the on-disk `playback` block doesn't reference a track that was just
/// removed. No-op otherwise.
fn sync_queue_with_playlist(playlist_id: i64, cx: &mut App) {
    let services = cx.global::<Services>();
    let matches = matches!(
        services.playback_queue.borrow().source(),
        crate::playback_queue::QueueSource::Playlist(id) if id == playlist_id,
    );
    if !matches {
        return;
    }
    let new_tracks = services.library.tracks_for_playlist(playlist_id);
    services
        .playback_queue
        .borrow_mut()
        .refresh_keeping_current(new_tracks.into_iter().map(Rc::new).collect());
    save_playback(cx);
}

fn advance_on_track_end(cx: &mut App) {
    let stop = crate::sleep_timer::stop_at_track_end(cx);
    let services = cx.global::<Services>();
    let next = services.playback_queue.borrow_mut().next_track().cloned();
    if let Some(track) = next {
        if stop {
            services.load_track(&track, cx);
        } else {
            services.play_track_gapless(&track, cx);
        }
        save_playback(cx);
    }
}

fn remap_queue_after_rescan(cx: &mut App) {
    let services = cx.global::<Services>().clone();
    let keys: Vec<(String, i32)> = {
        let queue = services.playback_queue.borrow();
        if queue.is_empty() {
            return;
        }
        let mut keys: Vec<(String, i32)> = queue
            .tracks_vec()
            .iter()
            .map(|t| (t.path.clone(), t.start_offset_ms))
            .collect();
        if let Some(orig) = queue.original_order_vec() {
            keys.extend(orig.iter().map(|t| (t.path.clone(), t.start_offset_ms)));
        }
        keys
    };

    let fresh: std::collections::HashMap<(String, i32), Rc<Track>> = services
        .library
        .tracks_by_keys(&keys)
        .into_iter()
        .map(|t| ((t.path.clone(), t.start_offset_ms), Rc::new(t)))
        .collect();

    if fresh.is_empty() {
        return;
    }

    services
        .playback_queue
        .borrow_mut()
        .remap_to_fresh_tracks(&fresh);
    save_playback(cx);
}

pub fn save_playback(cx: &mut App) {
    let state = cx.global::<Services>().snapshot_playback();
    if let Err(e) = cx
        .global_mut::<crate::settings_store::SettingsStore>()
        .set_playback(state)
    {
        crate::settings_store::notify_save_error(cx, e);
    }
}

pub fn apply_remote_state(cx: &mut App) {
    let store = cx.global::<crate::settings_store::SettingsStore>();
    let enabled = store.remote_enabled();
    let port = store.remote_port();
    let services = cx.global::<Services>().clone();

    let ready = {
        let mut slot = services.remote_server.borrow_mut();
        *slot = None;
        if enabled {
            let (server, ready) = pulse_remote::spawn(
                std::net::SocketAddr::from(([0, 0, 0, 0], port)),
                services.remote_state_rx.clone(),
                services.remote_command_tx.clone(),
                Arc::new(services.library.library_access()),
            );
            *slot = Some(server);
            Some(ready)
        } else {
            None
        }
    };

    if enabled {
        force_publish_remote_state(cx);
    } else {
        services
            .remote_handle
            .publish(pulse_remote::PlayerState::idle());
    }

    if let Some(ready) = ready {
        cx.spawn(async move |cx| {
            if let Ok(Err(err)) = ready.await {
                cx.update(|cx| notify_remote_error(cx, port, &err));
            }
        })
        .detach();
    }
}

fn notify_remote_error(cx: &mut App, port: u16, err: &str) {
    let s = crate::localization::tr();
    let Some(handle) = cx.windows().into_iter().next() else {
        return;
    };
    let message = s.remote_start_failed(port, err);
    let title = s.settings.clone();
    let _ = handle.update(cx, |_, window, cx| {
        window.push_notification(Notification::error(message).title(title), cx);
    });
}

pub fn toggle_play_pause(cx: &mut App) -> Option<bool> {
    let services = cx.global::<Services>();
    let current = services.playback_queue.borrow().current_track().cloned()?;
    let was_playing = services.is_playing.fetch_xor(true, Ordering::Relaxed);
    if was_playing {
        services.engine_manager.pause();
    } else {
        resume_or_load(services, &current, cx);
    }
    Some(!was_playing)
}

pub fn play(cx: &mut App) {
    let services = cx.global::<Services>();
    let Some(current) = services.playback_queue.borrow().current_track().cloned() else {
        return;
    };
    services.is_playing.store(true, Ordering::Relaxed);
    resume_or_load(services, &current, cx);
}

fn resume_or_load(services: &Services, current: &Track, cx: &App) {
    if services.current_duration_ms.load(Ordering::Relaxed) == 0 {
        services.play_track(current, cx);
    } else {
        services.engine_manager.play();
    }
}

pub fn pause(cx: &mut App) {
    let services = cx.global::<Services>();
    services.is_playing.store(false, Ordering::Relaxed);
    services.engine_manager.pause();
}

pub fn play_next(cx: &mut App) {
    let services = cx.global::<Services>();
    let next = services.playback_queue.borrow_mut().skip_to_next().cloned();
    if let Some(track) = next {
        services.play_track(&track, cx);
        save_playback(cx);
    }
}

pub fn play_previous(cx: &mut App) {
    let services = cx.global::<Services>();
    let position_secs = services.current_position_ms.load(Ordering::Relaxed) as f32 / 1000.0;
    let previous = {
        let mut queue = services.playback_queue.borrow_mut();
        match queue.previous(position_secs) {
            crate::playback_queue::PreviousAction::SeekToStart => None,
            crate::playback_queue::PreviousAction::PreviousTrack(track) => Some(track.clone()),
        }
    };
    match previous {
        None => {
            services.engine_manager.seek(0.0);
            services.engine_manager.play();
        }
        Some(track) => {
            services.play_track(&track, cx);
            save_playback(cx);
        }
    }
}

pub fn seek_to_ms(cx: &mut App, position_ms: u64) {
    let services = cx.global::<Services>();
    if services.playback_queue.borrow().current_track().is_none() {
        return;
    }
    let duration = services.current_duration_ms.load(Ordering::Relaxed);
    if duration == 0 {
        return;
    }
    let clamped = position_ms.min(duration);
    let ratio = (clamped as f32 / duration as f32).clamp(0.0, 1.0);
    services
        .current_position_ms
        .store(clamped, Ordering::Relaxed);
    services.engine_manager.seek(ratio);
}

pub fn play_queue_index(cx: &mut App, index: usize) {
    let services = cx.global::<Services>();
    let track = services
        .playback_queue
        .borrow_mut()
        .play_track_at(index)
        .cloned();
    if let Some(track) = track {
        services.play_track(&track, cx);
        save_playback(cx);
    }
}

fn remove_queue_index(cx: &mut App, index: usize) {
    let services = cx.global::<Services>();
    let outcome = services.playback_queue.borrow_mut().remove_track_at(index);
    match outcome {
        crate::playback_queue::RemoveOutcome::PlayNext(next) => {
            if services.is_playing.load(Ordering::Relaxed) {
                services.play_track(&next, cx);
            } else {
                services.load_track(&next, cx);
            }
        }
        crate::playback_queue::RemoveOutcome::Stopped => {
            services.current_position_ms.store(0, Ordering::Relaxed);
            services.stop_playback();
        }
        crate::playback_queue::RemoveOutcome::Unaffected => {}
    }
    queue_mutated(cx);
}

fn apply_remote_command(cx: &mut App, command: pulse_remote::Command) {
    match command {
        pulse_remote::Command::PlayPause => {
            toggle_play_pause(cx);
        }
        pulse_remote::Command::Next => play_next(cx),
        pulse_remote::Command::Prev => play_previous(cx),
        pulse_remote::Command::Seek { position_ms } => seek_to_ms(cx, position_ms),
        pulse_remote::Command::PlayAt { index } => play_queue_index(cx, index),
        pulse_remote::Command::RemoveAt { index } => remove_queue_index(cx, index),
        pulse_remote::Command::Refresh => force_publish_remote_state(cx),
        pulse_remote::Command::SetShuffle { on } => set_shuffle(cx, on),
        pulse_remote::Command::SetRepeat { mode } => set_repeat(cx, repeat_from_remote(mode)),
        pulse_remote::Command::SetVolume { volume } => set_volume(cx, volume),
        pulse_remote::Command::PlayArtistTrack {
            artist_id,
            track_id,
            full,
        } => play_artist_track(cx, artist_id, track_id, full),
        pulse_remote::Command::QueueArtistTrack {
            artist_id,
            track_id,
            full,
        } => queue_artist_track(cx, artist_id, track_id, full),
        pulse_remote::Command::QueueArtistAlbum {
            artist_id,
            album_id,
            full,
        } => queue_artist_album(cx, artist_id, album_id, full),
        pulse_remote::Command::PlayPlaylistTrack {
            playlist_id,
            track_id,
        } => play_playlist_track(cx, playlist_id, track_id),
        pulse_remote::Command::QueuePlaylistTrack {
            playlist_id,
            track_id,
        } => queue_playlist_track(cx, playlist_id, track_id),
        pulse_remote::Command::QueuePlaylist { playlist_id } => queue_playlist(cx, playlist_id),
        pulse_remote::Command::PlayFolderTrack {
            path,
            track_id,
            file,
        } => play_folder_track(cx, &path, track_id, file.as_deref()),
        pulse_remote::Command::QueueFolderTrack {
            path,
            track_id,
            file,
        } => queue_folder_track(cx, &path, track_id, file.as_deref()),
        pulse_remote::Command::QueueFolder { path } => queue_folder(cx, &path),
        pulse_remote::Command::PlayAlbum { album_id } => play_album(cx, album_id),
        pulse_remote::Command::QueueAlbum { album_id } => queue_album(cx, album_id),
        pulse_remote::Command::PlayAlbumTrack {
            album_id,
            track_id,
        } => play_album_track(cx, album_id, track_id),
        pulse_remote::Command::QueueAlbumTrack {
            album_id,
            track_id,
        } => queue_album_track(cx, album_id, track_id),
        pulse_remote::Command::PlayAllTrack { track_id } => play_all_track(cx, track_id),
        pulse_remote::Command::QueueAllTrack { track_id } => queue_all_track(cx, track_id),
        pulse_remote::Command::PlayNextTrack { track_id } => play_next_track(cx, track_id),
    }
}

fn play_artist_track(cx: &mut App, artist_id: i64, track_id: i64, full: bool) {
    let library = cx.global::<Services>().library.clone();
    let tracks = library.artist_display_tracks(artist_id, full);
    let Some(index) = tracks.iter().position(|t| t.id == track_id) else {
        return;
    };
    let tracks: Vec<Rc<Track>> = tracks.into_iter().map(Rc::new).collect();
    let name = library
        .artist_summary(
            artist_id,
            cx.global::<Services>().library.artists_grouping(),
        )
        .map(|artist| artist.name);
    crate::track_list::replace_queue_and_play(
        tracks,
        index,
        crate::playback_queue::QueueSource::Unknown,
        name.map(SharedString::from),
        cx,
    );
}

fn queue_artist_track(cx: &mut App, artist_id: i64, track_id: i64, full: bool) {
    let track = cx
        .global::<Services>()
        .library
        .artist_display_tracks(artist_id, full)
        .into_iter()
        .find(|t| t.id == track_id);
    if let Some(track) = track {
        crate::track_list::append_tracks_to_queue(vec![Rc::new(track)], cx);
    }
}

fn queue_artist_album(cx: &mut App, artist_id: i64, album_id: Option<i64>, full: bool) {
    let tracks: Vec<Rc<Track>> = cx
        .global::<Services>()
        .library
        .artist_display_tracks(artist_id, full)
        .into_iter()
        .filter(|t| t.album_id == album_id)
        .map(Rc::new)
        .collect();
    crate::track_list::append_tracks_to_queue(tracks, cx);
}

fn play_playlist_track(cx: &mut App, playlist_id: i64, track_id: i64) {
    let library = cx.global::<Services>().library.clone();
    let tracks = library.tracks_for_playlist(playlist_id);
    let Some(index) = tracks.iter().position(|t| t.id == track_id) else {
        return;
    };
    let tracks: Vec<Rc<Track>> = tracks.into_iter().map(Rc::new).collect();
    let name = library
        .playlists()
        .into_iter()
        .find(|playlist| playlist.id == playlist_id)
        .map(|playlist| playlist.name);
    crate::track_list::replace_queue_and_play(
        tracks,
        index,
        crate::playback_queue::QueueSource::Playlist(playlist_id),
        name.map(SharedString::from),
        cx,
    );
}

fn queue_playlist_track(cx: &mut App, playlist_id: i64, track_id: i64) {
    let track = cx
        .global::<Services>()
        .library
        .tracks_for_playlist(playlist_id)
        .into_iter()
        .find(|t| t.id == track_id);
    if let Some(track) = track {
        crate::track_list::append_tracks_to_queue(vec![Rc::new(track)], cx);
    }
}

/// The album's tracks, or nothing if the album has none left.
fn album_tracks(cx: &App, album_id: i64) -> Vec<Rc<Track>> {
    cx.global::<Services>()
        .library
        .tracks_for_album(album_id)
        .into_iter()
        .map(Rc::new)
        .collect()
}

fn album_name(cx: &App, album_id: i64) -> Option<SharedString> {
    cx.global::<Services>()
        .library
        .albums()
        .into_iter()
        .find(|album| album.id == album_id)
        .map(|album| album.title)
        .filter(|title| !title.is_empty())
        .map(SharedString::from)
}

fn play_album(cx: &mut App, album_id: i64) {
    let tracks = album_tracks(cx, album_id);
    if tracks.is_empty() {
        return;
    }
    let name = album_name(cx, album_id);
    crate::track_list::replace_queue_and_play(
        tracks,
        0,
        crate::playback_queue::QueueSource::Unknown,
        name,
        cx,
    );
}

fn queue_album(cx: &mut App, album_id: i64) {
    let tracks = album_tracks(cx, album_id);
    if !tracks.is_empty() {
        crate::track_list::append_tracks_to_queue(tracks, cx);
    }
}

fn play_album_track(cx: &mut App, album_id: i64, track_id: i64) {
    let tracks = album_tracks(cx, album_id);
    let Some(index) = tracks.iter().position(|track| track.id == track_id) else {
        return;
    };
    let name = album_name(cx, album_id);
    crate::track_list::replace_queue_and_play(
        tracks,
        index,
        crate::playback_queue::QueueSource::Unknown,
        name,
        cx,
    );
}

fn queue_album_track(cx: &mut App, album_id: i64, track_id: i64) {
    let track = album_tracks(cx, album_id)
        .into_iter()
        .find(|track| track.id == track_id);
    if let Some(track) = track {
        crate::track_list::append_tracks_to_queue(vec![track], cx);
    }
}

fn play_folder_track(cx: &mut App, path: &str, track_id: i64, file: Option<&str>) {
    let tracks = cx.global::<Services>().library.tracks_in_folder(path);
    let Some(index) = tracks.iter().position(|t| folder_row_is(t, track_id, file)) else {
        return;
    };
    let tracks: Vec<Rc<Track>> = tracks.into_iter().map(Rc::new).collect();
    let name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    crate::track_list::replace_queue_and_play(
        tracks,
        index,
        crate::playback_queue::QueueSource::Unknown,
        name.map(SharedString::from),
        cx,
    );
}

fn queue_folder_track(cx: &mut App, path: &str, track_id: i64, file: Option<&str>) {
    let track = cx
        .global::<Services>()
        .library
        .tracks_in_folder(path)
        .into_iter()
        .find(|t| folder_row_is(t, track_id, file));
    if let Some(track) = track {
        crate::track_list::append_tracks_to_queue(vec![Rc::new(track)], cx);
    }
}

/// A folder row is one file. Two copies of a song adopted from two folders are
/// two rows with one id, so the file disambiguates; clients that do not send it
/// get the first match, which is what the id-only lookup used to do.
fn folder_row_is(track: &Track, track_id: i64, file: Option<&str>) -> bool {
    track.id == track_id && file.is_none_or(|file| track.path == file)
}

fn queue_folder(cx: &mut App, path: &str) {
    let tracks: Vec<Rc<Track>> = cx
        .global::<Services>()
        .library
        .tracks_in_folder(path)
        .into_iter()
        .map(Rc::new)
        .collect();
    crate::track_list::append_tracks_to_queue(tracks, cx);
}

fn play_all_track(cx: &mut App, track_id: i64) {
    let tracks = cx.global::<Services>().library.all_tracks();
    let Some(index) = tracks.iter().position(|t| t.id == track_id) else {
        return;
    };
    let tracks: Vec<Rc<Track>> = tracks.into_iter().map(Rc::new).collect();
    crate::track_list::replace_queue_and_play(
        tracks,
        index,
        crate::playback_queue::QueueSource::AllTracks,
        Some(crate::localization::tr().all_tracks.clone()),
        cx,
    );
}

fn queue_all_track(cx: &mut App, track_id: i64) {
    let track = cx
        .global::<Services>()
        .library
        .all_tracks()
        .into_iter()
        .find(|t| t.id == track_id);
    if let Some(track) = track {
        crate::track_list::append_tracks_to_queue(vec![Rc::new(track)], cx);
    }
}

fn play_next_track(cx: &mut App, track_id: i64) {
    let track = cx
        .global::<Services>()
        .library
        .all_tracks()
        .into_iter()
        .find(|t| t.id == track_id);
    if let Some(track) = track {
        crate::track_list::play_tracks_next(vec![Rc::new(track)], cx);
    }
}

fn queue_playlist(cx: &mut App, playlist_id: i64) {
    let tracks: Vec<Rc<Track>> = cx
        .global::<Services>()
        .library
        .tracks_for_playlist(playlist_id)
        .into_iter()
        .map(Rc::new)
        .collect();
    crate::track_list::append_tracks_to_queue(tracks, cx);
}

pub fn toggle_shuffle(cx: &mut App) {
    let on = !cx.global::<Services>().playback_queue.borrow().shuffle();
    set_shuffle(cx, on);
}

pub fn set_shuffle(cx: &mut App, on: bool) {
    {
        let services = cx.global::<Services>();
        let mut queue = services.playback_queue.borrow_mut();
        if queue.shuffle() == on {
            return;
        }
        queue.set_shuffle(on);
    }
    playback_mode_changed(cx);
    queue_mutated(cx);
}

pub fn cycle_repeat(cx: &mut App) {
    let next = cx
        .global::<Services>()
        .playback_queue
        .borrow()
        .repeat()
        .cycle();
    set_repeat(cx, next);
}

pub fn set_repeat(cx: &mut App, mode: crate::playback_queue::RepeatMode) {
    {
        let services = cx.global::<Services>();
        let mut queue = services.playback_queue.borrow_mut();
        if queue.repeat() == mode {
            return;
        }
        queue.set_repeat(mode);
    }
    playback_mode_changed(cx);
    save_playback(cx);
    publish_remote_state(cx);
}

pub fn set_volume(cx: &mut App, volume: f32) {
    if !volume.is_finite() {
        return;
    }
    let services = cx.global::<Services>();
    if services.output.is_exclusive() {
        return;
    }
    let volume = volume.clamp(0.0, 1.0);
    services.output.set_volume(volume);
    if let Err(e) = cx
        .global_mut::<crate::settings_store::SettingsStore>()
        .set_volume(volume)
    {
        crate::settings_store::notify_save_error(cx, e);
    }
    publish_remote_state(cx);
}

fn playback_mode_changed(cx: &mut App) {
    let bus = cx.global::<Services>().library_event_bus.clone();
    bus.update(cx, |_, cx| cx.emit(LibraryEvent::PlaybackModeChanged));
}

fn repeat_to_remote(mode: crate::playback_queue::RepeatMode) -> pulse_remote::RepeatMode {
    match mode {
        crate::playback_queue::RepeatMode::Off => pulse_remote::RepeatMode::Off,
        crate::playback_queue::RepeatMode::All => pulse_remote::RepeatMode::All,
        crate::playback_queue::RepeatMode::One => pulse_remote::RepeatMode::One,
    }
}

fn repeat_from_remote(mode: pulse_remote::RepeatMode) -> crate::playback_queue::RepeatMode {
    match mode {
        pulse_remote::RepeatMode::Off => crate::playback_queue::RepeatMode::Off,
        pulse_remote::RepeatMode::All => crate::playback_queue::RepeatMode::All,
        pulse_remote::RepeatMode::One => crate::playback_queue::RepeatMode::One,
    }
}

pub fn queue_mutated(cx: &mut App) {
    let bus = cx.global::<Services>().library_event_bus.clone();
    bus.update(cx, |_, cx| cx.emit(LibraryEvent::QueueChanged));
    save_playback(cx);
    publish_remote_state(cx);
}

pub fn publish_remote_state(cx: &mut App) {
    if !cx.global::<Services>().remote_handle.is_serving() {
        return;
    }
    force_publish_remote_state(cx);
}

fn force_publish_remote_state(cx: &mut App) {
    let state = build_remote_state(cx);
    cx.global::<Services>().remote_handle.publish(state);
}

impl Global for Services {}

pub struct EngineEventsBus;

impl EngineEventsBus {}

impl EventEmitter<EngineEvent> for EngineEventsBus {}
impl Global for EngineEventsBus {}

pub struct LibraryEventsBus;

impl LibraryEventsBus {}

impl EventEmitter<LibraryEvent> for LibraryEventsBus {}
impl Global for LibraryEventsBus {}

#[allow(clippy::too_many_arguments)]
pub async fn run_engine_events_bus(
    cx: &mut AsyncApp,
    engine_manager: Rc<EngineManager>,
    engine_event_bus: Entity<EngineEventsBus>,
    current_position_ms: Arc<AtomicU64>,
    current_duration_ms: Arc<AtomicU64>,
    current_dsd_rate: Arc<AtomicU32>,
    is_playing: Arc<AtomicBool>,
    remote: pulse_remote::StateHandle,
) {
    let mut current_duration: Option<Duration> = None;
    let mut prefetched = false;
    let rx = engine_manager.events();
    while let Ok(event) = rx.recv_async().await {
        match &event {
            EngineEvent::Preparing { duration } => {
                current_duration = None;
                current_position_ms.store(0, Ordering::Relaxed);
                current_duration_ms.store(
                    duration.map_or(0, |d| d.as_millis() as u64),
                    Ordering::Relaxed,
                );
                current_dsd_rate.store(0, Ordering::Relaxed);
                publish_now_playing(cx);
            }
            EngineEvent::Loaded { params, duration } => {
                current_duration = Some(*duration);
                current_duration_ms.store(duration.as_millis() as u64, Ordering::Relaxed);
                current_dsd_rate.store(params.dsd_rate.unwrap_or(0), Ordering::Relaxed);
                prefetched = false;
                let duration_ms = duration.as_millis() as u64;
                cx.update(|cx| resume_restored_position(cx, duration_ms));
                publish_now_playing(cx);
            }
            EngineEvent::PositionChanged(dur) => {
                current_position_ms.store(dur.as_millis() as u64, Ordering::Relaxed);
                if remote.is_serving() {
                    remote.publish_position(
                        dur.as_millis() as u64,
                        is_playing.load(Ordering::Relaxed),
                    );
                }
                maybe_prefetch_next_track(cx, dur, current_duration, &mut prefetched);
            }
            EngineEvent::Playing => {
                is_playing.store(true, Ordering::Relaxed);
                publish_now_playing(cx);
            }
            EngineEvent::Paused => {
                is_playing.store(false, Ordering::Relaxed);
                publish_now_playing(cx);
            }
            EngineEvent::TrackEnded => {
                is_playing.store(false, Ordering::Relaxed);
                cx.update(advance_on_track_end);
                publish_now_playing(cx);
            }
            EngineEvent::Stopped => {
                is_playing.store(false, Ordering::Relaxed);
                current_position_ms.store(0, Ordering::Relaxed);
                current_duration_ms.store(0, Ordering::Relaxed);
                current_dsd_rate.store(0, Ordering::Relaxed);
                publish_now_playing(cx);
            }
            EngineEvent::Buffering(buffering) => {
                let buffering = *buffering;
                cx.update(|cx| {
                    cx.global::<Services>()
                        .is_buffering
                        .store(buffering, Ordering::Relaxed)
                });
                publish_now_playing(cx);
            }
            EngineEvent::Error(message) => {
                is_playing.store(false, Ordering::Relaxed);
                current_duration_ms.store(0, Ordering::Relaxed);
                let message = message.clone();
                cx.update(|cx| {
                    let Some(handle) = cx.windows().into_iter().next() else {
                        return;
                    };
                    let _ = handle.update(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(message)
                                .title(crate::localization::tr().playback_failed_title.clone()),
                            cx,
                        );
                    });
                });
                publish_now_playing(cx);
            }
        }
        cx.update(|cx| engine_event_bus.update(cx, |_, cx| cx.emit(event)));
    }
}

fn resume_restored_position(cx: &mut App, duration_ms: u64) {
    let services = cx.global::<Services>();
    let Some((track_id, position_ms)) = services.resume_at.take() else {
        return;
    };
    let current = services
        .playback_queue
        .borrow()
        .current_track()
        .map(|t| t.id);
    if current != Some(track_id) || duration_ms == 0 || position_ms >= duration_ms {
        return;
    }
    services
        .current_position_ms
        .store(position_ms, Ordering::Relaxed);
    services
        .engine_manager
        .seek(position_ms as f32 / duration_ms as f32);
}

fn publish_now_playing(cx: &mut AsyncApp) {
    cx.update(publish_remote_state);
}

fn build_remote_state(cx: &mut App) -> pulse_remote::PlayerState {
    use std::hash::{Hash, Hasher};

    let services = cx.global::<Services>();
    let queue = services.playback_queue.borrow();
    let tracks = queue.tracks_vec();
    let queue_index = queue.current_index();
    let shuffle = queue.shuffle();
    let repeat = repeat_to_remote(queue.repeat());
    let current = queue.current_track().map(|t| {
        (
            t.id,
            t.title.clone(),
            t.album_id,
            t.cover_art_id,
            t.duration_ms.unwrap_or(0).max(0) as u64,
        )
    });
    drop(queue);
    let volume = cx.global::<crate::settings_store::SettingsStore>().volume();
    let volume_locked = services.output.is_exclusive();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for t in tracks.iter() {
        t.id.hash(&mut hasher);
        t.cover_art_id.hash(&mut hasher);
        t.title.hash(&mut hasher);
    }
    let queue_rev = hasher.finish();
    let library_rev = services.library_rev.load(Ordering::Relaxed);

    let cached = services
        .remote_queue_cache
        .borrow()
        .as_ref()
        .filter(|(rev, _)| *rev == queue_rev)
        .map(|(_, items)| items.clone());
    let queue_items = cached.unwrap_or_else(|| {
        let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
        let artists = services.library.track_artists_map(&ids);
        let items: Arc<Vec<pulse_remote::QueueItem>> = Arc::new(
            tracks
                .iter()
                .map(|t| pulse_remote::QueueItem {
                    id: t.id,
                    title: t.title.clone(),
                    artist: artists.get(&t.id).and_then(|a| a.first().cloned()),
                    cover_id: t.cover_art_id,
                })
                .collect(),
        );
        *services.remote_queue_cache.borrow_mut() = Some((queue_rev, items.clone()));
        items
    });

    let Some((track_id, title, album_id, cover_id, duration_ms)) = current else {
        return pulse_remote::PlayerState {
            queue_index,
            queue_rev,
            library_rev,
            shuffle,
            repeat,
            volume,
            volume_locked,
            queue: queue_items,
            ..pulse_remote::PlayerState::idle()
        };
    };

    let artist = services.library.track_artists(track_id).into_iter().next();
    let album = album_id.and_then(|id| services.library.album_title(id));

    pulse_remote::PlayerState {
        v: pulse_remote::PROTOCOL_VERSION,
        has_track: true,
        title: Some(title),
        artist,
        album,
        playing: services.is_playing.load(Ordering::Relaxed),
        buffering: services.is_buffering.load(Ordering::Relaxed),
        position_ms: services.current_position_ms.load(Ordering::Relaxed),
        duration_ms,
        cover_id,
        queue_index,
        queue_rev,
        library_rev,
        shuffle,
        repeat,
        volume,
        volume_locked,
        queue: queue_items,
    }
}

/// If the next track is known and we're within 2 seconds of the end of the
/// current one, warm the OS page cache by reading the first 64 KiB of the next
/// track's file. This eliminates decoder-open latency for gapless transitions,
/// especially on spinning disks.
fn maybe_prefetch_next_track(
    cx: &AsyncApp,
    position: &Duration,
    track_duration: Option<Duration>,
    prefetched: &mut bool,
) {
    if *prefetched {
        return;
    }
    let near_end = track_duration
        .map(|d| d.saturating_sub(*position) <= Duration::from_secs(2))
        .unwrap_or(false);
    if !near_end {
        return;
    }
    *prefetched = true;

    let Some(path) = cx.update(|cx| {
        cx.global::<Services>()
            .playback_queue
            .borrow()
            .peek_next()
            .map(|t| std::path::PathBuf::from(&t.path))
    }) else {
        return;
    };

    cx.background_spawn(async move {
        if let Ok(mut file) = std::fs::File::open(&path) {
            let mut buf = [0u8; 65536];
            let _ = file.read(&mut buf);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_store::QueueRef;
    use music_library::{LibraryRepository, NewTrack, SqliteLibrary};

    fn repo_with(dir: &tempfile::TempDir, paths: &[&str]) -> (SqliteLibrary, Vec<i64>) {
        let lib = SqliteLibrary::open_at(dir.path().join("library.db")).unwrap();
        let artist = lib.upsert_artist("Artist").unwrap();
        let ids = paths
            .iter()
            .map(|path| {
                let track = NewTrack {
                    path: (*path).into(),
                    title: Some(path.rsplit('/').next().unwrap().into()),
                    ..Default::default()
                };
                lib.upsert_track(&track, None, &[(artist, 0)]).unwrap()
            })
            .collect();
        (lib, ids)
    }

    /// The whole point of storing a locator instead of a snapshot: a queue entry
    /// comes back as the *current* catalog row, and an entry the catalog cannot
    /// answer for still occupies its slot.
    #[test]
    fn a_restored_queue_resolves_by_id_then_path_then_gives_up_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let (lib, ids) = repo_with(&dir, &["/music/a.flac", "/music/b.flac"]);

        let refs = vec![
            // Straight id hit.
            QueueRef {
                id: ids[0],
                path: "/music/a.flac".into(),
                start_offset_ms: 0,
            },
            // Id the catalog never issued, saved path still good.
            QueueRef {
                id: 9_999_999,
                path: "/music/b.flac".into(),
                start_offset_ms: 0,
            },
            // Nothing matches: file is away, entry must survive.
            QueueRef {
                id: 9_999_998,
                path: "/gone/missing.flac".into(),
                start_offset_ms: 0,
            },
        ];

        let out = restore_queue(&refs, &lib);

        assert_eq!(out.len(), 3, "an unresolvable entry must not be dropped");
        assert_eq!(out[0].id, ids[0]);
        assert_eq!(out[0].title, "a.flac");
        assert_eq!(out[1].id, ids[1], "path fallback keeps the real id");
        assert_eq!(out[1].title, "b.flac");
        assert!(!out[2].available);
        assert_eq!(out[2].title, "missing");
        assert_eq!(out[2].path, "/gone/missing.flac");
    }

    #[test]
    fn an_empty_queue_restores_to_nothing_without_queries() {
        let dir = tempfile::tempdir().unwrap();
        let (lib, _) = repo_with(&dir, &[]);

        assert!(restore_queue(&[], &lib).is_empty());
    }

    /// A cue piece shares its path with its siblings and is told apart by the
    /// offset, so the path fallback has to keep them separate.
    #[test]
    fn a_restored_cue_piece_keeps_its_own_offset() {
        let dir = tempfile::tempdir().unwrap();
        let lib = SqliteLibrary::open_at(dir.path().join("library.db")).unwrap();
        let artist = lib.upsert_artist("Artist").unwrap();
        let mut ids = Vec::new();
        for (title, offset) in [("One", 0), ("Two", 90_000)] {
            ids.push(
                lib.upsert_track(
                    &NewTrack {
                        path: "/album/image.flac".into(),
                        title: Some(title.into()),
                        start_offset_ms: Some(offset),
                        ..Default::default()
                    },
                    None,
                    &[(artist, 0)],
                )
                .unwrap(),
            );
        }

        // Ids unknown, so both fall through to the path+offset lookup.
        let refs = vec![
            QueueRef {
                id: 9_999_997,
                path: "/album/image.flac".into(),
                start_offset_ms: 90_000,
            },
            QueueRef {
                id: 9_999_996,
                path: "/album/image.flac".into(),
                start_offset_ms: 0,
            },
        ];

        let out = restore_queue(&refs, &lib);

        assert_eq!(out[0].title, "Two");
        assert_eq!(out[1].title, "One");
        assert_eq!(out[0].start_offset_ms, 90_000);
    }
}
