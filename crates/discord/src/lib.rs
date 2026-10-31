mod art;
mod ipc;

use std::sync::OnceLock;
use std::time::Duration;

pub use ipc::DiscordHandle;

fn resolved_client_id() -> Option<&'static str> {
    static ID: OnceLock<Option<String>> = OnceLock::new();
    ID.get_or_init(|| {
        std::env::var("DISCORD_CLIENT_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| option_env!("DISCORD_CLIENT_ID").map(str::to_owned))
    })
    .as_deref()
}

pub fn client_id() -> Option<String> {
    resolved_client_id().map(str::to_owned)
}

pub fn is_available() -> bool {
    resolved_client_id().is_some()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration: Duration,
    pub started_at: i64,
    /// Playback is paused: the activity stays up but the timer is not running.
    pub paused: bool,
    /// What to include in the activity.
    pub settings: PresenceSettings,
}

/// What the user wants on their profile, and nothing else.
///
/// It lives here rather than in the app so the settings file and the activity
/// builder share one type: a field added on one side is a compile error on the
/// other, not a setting that silently does nothing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PresenceSettings {
    #[serde(default = "default_true")]
    pub show_title: bool,
    #[serde(default = "default_true")]
    pub show_artist: bool,
    #[serde(default = "default_true")]
    pub show_album: bool,
    #[serde(default)]
    pub show_cover: bool,
    #[serde(default = "default_true")]
    pub show_timestamps: bool,
    #[serde(default)]
    pub keep_on_pause: bool,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self {
            show_title: true,
            show_artist: true,
            show_album: true,
            show_cover: false,
            show_timestamps: true,
            keep_on_pause: false,
        }
    }
}

fn default_true() -> bool {
    true
}
