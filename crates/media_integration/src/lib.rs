#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::rc::Rc;

use flume::Sender;

mod souvlaki_backend;

/// Snapshot of the currently playing track for system media widgets.
#[derive(Debug, Clone, Default)]
pub struct NowPlayingInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork_path: Option<PathBuf>,
    pub duration_secs: f64,
    pub elapsed_secs: Option<f64>,
}

/// Playback state exposed to the OS media controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaPlaybackState {
    Playing,
    Paused,
    Stopped,
}

/// Commands the OS can send to the player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaCommand {
    Play,
    Pause,
    TogglePlayPause,
    Next,
    Previous,
    Seek(f64),
}

/// Platform-agnostic interface for system media integration.
pub trait SystemMediaIntegration {
    /// Update the metadata shown in the system Now Playing widget.
    fn update_now_playing(&self, info: NowPlayingInfo, state: MediaPlaybackState);

    /// Update the playback state shown in system UI.
    fn set_playback_state(&self, state: MediaPlaybackState);

    /// Update just the elapsed playback time in the Now Playing widget.
    /// Called frequently (e.g. every second) so it should be lightweight.
    fn update_position(&self, elapsed_secs: f64, state: MediaPlaybackState);
}

/// Create the system media integration (MPRIS over D-Bus).
///
/// Returns `None` when there is no session bus or it failed to initialize.
pub fn create_integration(
    command_sender: Sender<MediaCommand>,
) -> Option<Rc<dyn SystemMediaIntegration>> {
    let integration = souvlaki_backend::SouvlakiIntegration::new(command_sender)?;
    Some(Rc::new(integration))
}
