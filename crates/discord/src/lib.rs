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
    pub year: Option<i32>,
    pub duration: Duration,
    pub started_at: i64,
    /// Playback is paused: the activity stays up but the timer is not running.
    pub paused: bool,
    /// What to include in the activity.
    pub settings: PresenceSettings,
}

/// One of the things an activity line can show.
///
/// A Discord activity has two text lines and a tooltip on the artwork. They are
/// all filled from the same handful of fields, so "what goes on line one" and
/// "what goes on line two" are the same question asked twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Line {
    /// Nothing: the line is left out of the activity entirely.
    Off,
    #[default]
    Title,
    Artist,
    Album,
    Year,
}

/// How the activity's clock behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Timers {
    /// No clock at all.
    Off,
    /// A counter running from the start of the track.
    Elapsed,
    /// A progress bar: elapsed time against the track's length.
    #[default]
    Progress,
}

/// The verb Discord puts in front of the application name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Verb {
    #[default]
    Listening,
    Playing,
}

/// What Discord shows on the line above the activity's two text lines.
///
/// That line carries the application name unless the activity sets one. The
/// field is a free-form string, so the name can be replaced with a track field
/// or with a single space, which Discord renders as an empty line.
///
/// Clients that ignore the field keep showing "Pulse", which is why leaving it
/// alone is the default rather than something the user has to undo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Header {
    /// The application name.
    #[default]
    App,
    /// Nothing: the activity starts at the first text line.
    Off,
    Title,
    Artist,
    Album,
    Year,
}

impl Header {
    /// The [`Line`] this header reads, or `None` for "leave the field unset".
    fn line(self) -> Option<Line> {
        match self {
            Self::App => None,
            Self::Off => Some(Line::Off),
            Self::Title => Some(Line::Title),
            Self::Artist => Some(Line::Artist),
            Self::Album => Some(Line::Album),
            Self::Year => Some(Line::Year),
        }
    }
}

/// What the user wants on their profile, and nothing else.
///
/// It lives here rather than in the app so the settings file and the activity
/// builder share one type: a field added on one side is a compile error on the
/// other, not a setting that silently does nothing.
///
/// Fields added after the first release carry `#[serde(default)]`, and unknown
/// keys in an older settings file are ignored, so an upgrade keeps whatever the
/// user had set for the fields that still exist.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PresenceSettings {
    /// What Discord shows above the two lines: the app name by default.
    #[serde(default)]
    pub header: Header,
    /// What the first line shows.
    #[serde(default)]
    pub line_one: Line,
    /// What the second line shows.
    #[serde(default = "default_line_two")]
    pub line_two: Line,
    /// Show the album cover as the large image. Off by default: it is the only
    /// setting that makes the app request something over the network.
    #[serde(default)]
    pub show_cover: bool,
    /// The tooltip on the cover, for whoever hovers it.
    #[serde(default = "default_cover_text")]
    pub cover_text: Line,
    /// The clock under the activity.
    #[serde(default)]
    pub timers: Timers,
    /// "Listening to Pulse" or "Playing Pulse".
    #[serde(default)]
    pub verb: Verb,
    /// Keep the activity up while paused instead of clearing it.
    #[serde(default)]
    pub keep_on_pause: bool,
    /// While paused, put "Paused" on the second line. Without this the only
    /// sign of a pause is the small image, whose asset key has to be uploaded
    /// to the Discord application first — so by default it shows nothing.
    #[serde(default = "default_true")]
    pub paused_line: bool,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self {
            header: Header::App,
            line_one: Line::Title,
            line_two: Line::Artist,
            show_cover: false,
            cover_text: Line::Album,
            timers: Timers::Progress,
            verb: Verb::Listening,
            keep_on_pause: false,
            paused_line: true,
        }
    }
}

fn default_line_two() -> Line {
    Line::Artist
}

fn default_cover_text() -> Line {
    Line::Album
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_settings_file_keeps_the_switches_it_still_has() {
        // What 5.3 wrote: six booleans, two of which survived the redesign.
        let old = r#"{"show_title":true,"show_artist":true,"show_album":true,
                      "show_cover":true,"show_timestamps":false,"keep_on_pause":true}"#;
        let settings: PresenceSettings = serde_json::from_str(old).unwrap();

        assert!(settings.show_cover, "a surviving switch is kept");
        assert!(settings.keep_on_pause);
        assert_eq!(settings.line_one, Line::Title, "new fields take defaults");
        assert_eq!(settings.line_two, Line::Artist);
        assert_eq!(settings.timers, Timers::Progress);
        assert_eq!(settings.cover_text, Line::Album);
        assert!(settings.paused_line);
    }

    #[test]
    fn a_missing_settings_block_is_the_default() {
        assert_eq!(PresenceSettings::default().header, Header::App);
        assert_eq!(PresenceSettings::default().line_one, Line::Title);
        assert_eq!(PresenceSettings::default().line_two, Line::Artist);
        assert!(!PresenceSettings::default().show_cover);
        assert_eq!(PresenceSettings::default().timers, Timers::Progress);
        assert_eq!(PresenceSettings::default().verb, Verb::Listening);
    }

    #[test]
    fn the_header_defaults_to_the_app_name_for_a_file_written_before_it_existed() {
        let old = r#"{"line_one":"Title","line_two":"Artist"}"#;
        let settings: PresenceSettings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.header, Header::App);
    }
}
