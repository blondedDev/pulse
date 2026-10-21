use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{App, BackgroundExecutor, Global, Pixels, px};
use gpui_component::{
    WindowExt,
    notification::Notification,
    scroll::ScrollbarMode,
    theme::{Theme, ThemeMode},
};
use serde::{Deserialize, Serialize};

use crate::localization::tr;

fn default_true() -> bool {
    true
}

fn default_volume() -> f32 {
    1.0
}

fn default_remote_port() -> u16 {
    pulse_remote::DEFAULT_PORT
}

pub const LYRICS_FONT_SIZE_MIN: f32 = 12.;
pub const LYRICS_FONT_SIZE_MAX: f32 = 32.;
pub const LYRICS_FONT_SIZE_DEFAULT: f32 = 16.;

fn default_lyrics_font_size() -> f32 {
    LYRICS_FONT_SIZE_DEFAULT
}

pub const BLUR_INTENSITY_MIN: f32 = 0.;
pub const BLUR_INTENSITY_MAX: f32 = 20.;
/// 30% of the slider range. Enough to soften the backdrop without smearing
/// the cover into mush; this is the value the app was tuned against by hand.
pub const BLUR_INTENSITY_DEFAULT: f32 = 6.;

fn default_blur_intensity() -> f32 {
    BLUR_INTENSITY_DEFAULT
}

pub const BLUR_INTERFACE_OPACITY_MIN: f32 = 0.;
pub const BLUR_INTERFACE_OPACITY_MAX: f32 = 200.;
/// Fully transparent chrome over the backdrop: the panels stop painting a
/// surface and the blurred cover shows through, with only the foreground
/// colours left. Everywhere the backdrop is not drawn (every view while
/// `BlurBackground` is `CoverView` or `Off`) the ordinary opaque surfaces are
/// used, so this costs nothing until the blur is actually on.
pub const BLUR_INTERFACE_OPACITY_DEFAULT: f32 = 0.;

fn default_blur_interface_opacity() -> f32 {
    BLUR_INTERFACE_OPACITY_DEFAULT
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepeatModePersist {
    #[default]
    Off,
    All,
    One,
}

impl From<crate::playback_queue::RepeatMode> for RepeatModePersist {
    fn from(mode: crate::playback_queue::RepeatMode) -> Self {
        match mode {
            crate::playback_queue::RepeatMode::Off => Self::Off,
            crate::playback_queue::RepeatMode::All => Self::All,
            crate::playback_queue::RepeatMode::One => Self::One,
        }
    }
}

impl From<RepeatModePersist> for crate::playback_queue::RepeatMode {
    fn from(mode: RepeatModePersist) -> Self {
        match mode {
            RepeatModePersist::Off => Self::Off,
            RepeatModePersist::All => Self::All,
            RepeatModePersist::One => Self::One,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FontScale {
    #[default]
    Small,
    Medium,
    Large,
}

impl FontScale {
    pub fn px(self) -> Pixels {
        match self {
            FontScale::Small => px(16.),
            FontScale::Medium => px(19.),
            FontScale::Large => px(22.),
        }
    }

    pub fn ui_scale(self) -> f32 {
        match self {
            FontScale::Small => 1.0,
            FontScale::Medium => 1.1,
            FontScale::Large => 1.2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlurBackground {
    Off,
    #[default]
    CoverView,
    AllViews,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlbumsArtistDisplay {
    #[default]
    Inline,
    Column,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NowPlayingDetails {
    #[default]
    Specs,
    Year,
    Album,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlbumsLayout {
    List,
    #[default]
    Grid,
}

/// Which library tab is selected when the window first opens.
///
/// `Genres` is no longer offered — the tab is gone — but the variant stays so a
/// `settings.json` written before that still parses instead of being set aside
/// as unreadable. [`SettingsStore::startup_tab`] maps it to `Albums`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartupTab {
    #[default]
    Albums,
    Artists,
    Genres,
    AllTracks,
    Folders,
    Playlists,
}

/// How the main window is sized at launch.
///
/// `Landscape` and `Portrait` are fractions of the monitor's usable area rather
/// than fixed pixel sizes, so they stay sensible on a 1080p laptop and a 1440p
/// desktop alike. `Custom` uses `custom_window_width`/`custom_window_height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowPreset {
    #[default]
    Landscape,
    Portrait,
    Custom,
}

impl WindowPreset {
    /// Fraction of the monitor's usable width/height this preset occupies.
    pub fn aspect(self) -> Option<(f32, f32)> {
        match self {
            // Wide and short: the shape the queue + library layout suits. The
            // fractions are the size the window actually runs at on a 1080p
            // screen (1857 x 705 of a 1920-wide usable area), so the default
            // preset opens where the user left it.
            Self::Landscape => Some((0.967, 0.68)),
            // Tall and narrow: better for small side-by-side windows. The
            // width fraction is a wish, not a promise — `WINDOW_MIN_WIDTH`
            // floors it, so on a short screen this lands as "as tall as the
            // screen allows" rather than genuinely portrait.
            Self::Portrait => Some((0.45, 0.98)),
            Self::Custom => None,
        }
    }
}

pub const WINDOW_MIN_WIDTH: f32 = 900.;
pub const WINDOW_MIN_HEIGHT: f32 = 400.;

fn default_custom_window_width() -> f32 {
    1857.
}

fn default_custom_window_height() -> f32 {
    705.
}

/// A dimension typed into the settings: whole pixels, never below `min`, and
/// `min` rather than a nonsensical value when it is NaN or non-positive.
fn sane_dimension(value: f32, min: f32) -> f32 {
    if !value.is_finite() || value <= 0. {
        return min;
    }
    value.round().max(min)
}

/// Resolve a preset into concrete pixels for a monitor of `monitor_width` x
/// `monitor_height`, clamped to the monitor and to `WINDOW_MIN_*`.
///
/// `monitor_*` should be the *usable* area (taskbar and dock excluded) so the
/// window cannot open underneath a panel. Clamping is applied to the minimum
/// too: on a display smaller than the minimum, the minimum wins, because a
/// window that cannot be laid out is worse than one that overflows slightly.
pub fn resolve_window_size(
    preset: WindowPreset,
    custom_width: f32,
    custom_height: f32,
    monitor_width: f32,
    monitor_height: f32,
) -> (f32, f32) {
    let (mut w, mut h) = match preset.aspect() {
        Some((fw, fh)) => (monitor_width * fw, monitor_height * fh),
        None => (custom_width, custom_height),
    };
    if !w.is_finite() || w <= 0. {
        w = WINDOW_MIN_WIDTH;
    }
    if !h.is_finite() || h <= 0. {
        h = WINDOW_MIN_HEIGHT;
    }
    w = w.clamp(WINDOW_MIN_WIDTH, monitor_width.max(WINDOW_MIN_WIDTH));
    h = h.clamp(WINDOW_MIN_HEIGHT, monitor_height.max(WINDOW_MIN_HEIGHT));
    (w, h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum QueueSourcePersist {
    #[default]
    Unknown,
    Playlist(i64),
    AllTracks,
}

impl From<crate::playback_queue::QueueSource> for QueueSourcePersist {
    fn from(s: crate::playback_queue::QueueSource) -> Self {
        match s {
            crate::playback_queue::QueueSource::Unknown => Self::Unknown,
            crate::playback_queue::QueueSource::Playlist(id) => Self::Playlist(id),
            crate::playback_queue::QueueSource::AllTracks => Self::AllTracks,
        }
    }
}

impl From<QueueSourcePersist> for crate::playback_queue::QueueSource {
    fn from(s: QueueSourcePersist) -> Self {
        match s {
            QueueSourcePersist::Unknown => Self::Unknown,
            QueueSourcePersist::Playlist(id) => Self::Playlist(id),
            QueueSourcePersist::AllTracks => Self::AllTracks,
        }
    }
}

/// One queue entry as it is persisted.
///
/// The queue is nothing but catalog tracks, so only what is needed to *find* a
/// track again is stored. Everything else — title, album, cover, duration —
/// is re-read from the database on load, which keeps it current: re-tagging a
/// track and restarting shows the new title instead of the copy that was frozen
/// when it was queued.
///
/// Older builds wrote a whole `Track` here. Serde ignores the surplus fields, so
/// those files deserialize into this type unchanged and are resolved the same
/// way, which is why the change needed no version marker and no migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueRef {
    /// `media_items.id` — durable across rescans and across a file moving, so it
    /// is tried before the path.
    pub id: i64,
    /// The path it was queued from. Used when the id no longer resolves, and as
    /// the locator for a track the catalog does not know any more.
    pub path: String,
    #[serde(default)]
    pub start_offset_ms: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackState {
    #[serde(default)]
    pub queue: Vec<QueueRef>,
    #[serde(default)]
    pub original_queue: Option<Vec<QueueRef>>,
    #[serde(default)]
    pub current_index: Option<usize>,
    #[serde(default)]
    pub position_ms: u64,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: RepeatModePersist,
    #[serde(default)]
    pub source: QueueSourcePersist,
    /// The display name for `source` ("Astroworld", "Liked Songs"). Stored
    /// alongside it because resolving it needs the catalog, and the queue is
    /// restored before anything has asked the catalog a question.
    #[serde(default)]
    pub source_name: Option<String>,
    #[serde(default)]
    pub custom: bool,
}

/// Which animations are allowed to run.
///
/// One struct rather than a handful of loose booleans, so the Animations page
/// can list them from a single place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub panel_slide: bool,
    #[serde(default = "default_true")]
    pub view_fade: bool,
    #[serde(default = "default_true")]
    pub track_change: bool,
    #[serde(default = "default_true")]
    pub cover_mode: bool,
    /// The Settings screen rising into place as it opens.
    #[serde(default = "default_true")]
    pub settings_open: bool,
    /// Album covers fading in as they finish loading.
    #[serde(default = "default_true")]
    pub cover_fade: bool,
}

impl Default for AnimationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            panel_slide: true,
            view_fade: true,
            track_change: true,
            cover_mode: true,
            settings_open: true,
            cover_fade: true,
        }
    }
}

/// Which ReplayGain value to apply.
///
/// Off by default: it changes how loud the music is, which is not something to
/// do to somebody's library without them asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayGainMode {
    #[default]
    Off,
    /// Each track to the same perceived loudness. Changes the relative level of
    /// tracks within an album.
    Track,
    /// Each album to the same perceived loudness, keeping its internal dynamics.
    Album,
}

impl ReplayGainMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Track, Self::Album];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub dynamic_theme: bool,
    #[serde(default)]
    pub music_folders: Vec<PathBuf>,
    /// Folders the duplicate finder may act on, best first. Only these are
    /// ranked *and* only these are ever deleted from.
    #[serde(default)]
    pub duplicate_priority: Vec<String>,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub replaygain: ReplayGainMode,
    /// Cap a positive gain at the headroom the track's peak allows.
    #[serde(default = "default_true")]
    pub replaygain_prevent_clipping: bool,
    #[serde(default)]
    pub playback: PlaybackState,
    #[serde(default)]
    pub show_hog_button: bool,
    #[serde(default)]
    pub show_device_picker: bool,
    #[serde(default = "default_true")]
    pub show_repeat_shuffle: bool,
    #[serde(default = "default_true")]
    pub show_time_labels: bool,
    #[serde(default)]
    pub now_playing_details: NowPlayingDetails,
    // Album and Artist can be hidden like the rest; All Tracks cannot — it is
    // the fallback every other toggle resolves to.
    #[serde(default = "default_true")]
    pub albums_enabled: bool,
    #[serde(default = "default_true")]
    pub artists_enabled: bool,
    #[serde(default = "default_true")]
    pub playlists_enabled: bool,
    #[serde(default = "default_true")]
    pub rewind_enabled: bool,
    #[serde(default)]
    pub animations: AnimationSettings,
    #[serde(default = "default_true")]
    pub export_include_settings: bool,
    #[serde(default)]
    pub export_include_library: bool,
    #[serde(default = "default_true")]
    pub folders_enabled: bool,
    // The Tools page was removed; sleep timer lives in Settings now. The key is
    // still accepted so an older settings.json loads, and dropped on the next save.
    #[serde(default, skip_serializing)]
    _legacy_tools_enabled: bool,
    #[serde(default = "default_true")]
    pub show_track_duration: bool,
    #[serde(default = "default_true")]
    pub show_queue_actions: bool,
    #[serde(default = "default_true")]
    pub show_queue_artist: bool,
    #[serde(default = "default_true")]
    pub cover_show_artist: bool,
    #[serde(default = "default_true")]
    pub cover_show_progress: bool,
    #[serde(default = "default_true")]
    pub cover_show_controls: bool,
    #[serde(default)]
    pub blur_background: BlurBackground,
    #[serde(default = "default_blur_intensity")]
    pub blur_intensity: f32,
    #[serde(default = "default_blur_interface_opacity")]
    pub blur_interface_opacity: f32,
    #[serde(default)]
    pub queue_deduplication: bool,
    #[serde(default = "default_true")]
    pub tag_editor_enabled: bool,
    #[serde(default = "default_true")]
    pub albums_show_year: bool,
    #[serde(default = "default_true")]
    pub albums_show_genre: bool,
    #[serde(default)]
    pub albums_artist_display: AlbumsArtistDisplay,
    #[serde(default)]
    pub albums_layout: AlbumsLayout,
    #[serde(default)]
    pub startup_tab: StartupTab,
    #[serde(default)]
    pub window_preset: WindowPreset,
    #[serde(default = "default_custom_window_width")]
    pub custom_window_width: f32,
    #[serde(default = "default_custom_window_height")]
    pub custom_window_height: f32,
    #[serde(default)]
    pub artists_grouping: music_library::ArtistGrouping,
    #[serde(default = "default_true")]
    pub lyrics_from_internet: bool,
    #[serde(default)]
    pub lyrics_prefer_lrclib: bool,
    #[serde(default)]
    pub remote_enabled: bool,
    #[serde(default = "default_remote_port")]
    pub remote_port: u16,
    // Torrent and the network-cache size were removed. These stay so an older
    // settings.json still deserializes instead of erroring out; they are never
    // read and get dropped on the next save.
    #[serde(default, rename = "torrent_sources", skip_serializing)]
    _legacy_torrent_sources: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "torrent_upload", skip_serializing)]
    _legacy_torrent_upload: Option<serde_json::Value>,
    #[serde(default, rename = "network_cache_gb", skip_serializing)]
    _legacy_network_cache_gb: Option<u32>,
    // Media servers (Subsonic / Jellyfin / DLNA) were removed in 3.0. The keys
    // stay as legacy placeholders so an old settings.json still loads; nothing
    // reads them and they are dropped on the next save.
    #[serde(default, rename = "subsonic_servers", skip_serializing)]
    _legacy_subsonic_servers: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "jellyfin_servers", skip_serializing)]
    _legacy_jellyfin_servers: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "dlna_servers", skip_serializing)]
    _legacy_dlna_servers: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub discord_enabled: bool,
    #[serde(default)]
    pub font_scale: FontScale,
    #[serde(default = "default_lyrics_font_size")]
    pub lyrics_font_size: f32,
    #[serde(default = "default_true")]
    pub lyrics_karaoke_fill: bool,
    #[serde(default = "default_true")]
    pub lyrics_dim_inactive: bool,
    #[serde(default)]
    pub onboarding_complete: bool,
    #[serde(default)]
    pub sleep_timer: SleepTimerSettings,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            dynamic_theme: false,
            music_folders: Vec::new(),
            duplicate_priority: Vec::new(),
            volume: 1.0,
            replaygain: ReplayGainMode::Off,
            replaygain_prevent_clipping: true,
            playback: PlaybackState::default(),
            show_hog_button: false,
            show_device_picker: false,
            show_repeat_shuffle: true,
            show_time_labels: true,
            now_playing_details: NowPlayingDetails::default(),
            albums_enabled: true,
            artists_enabled: true,
            playlists_enabled: true,
            rewind_enabled: true,
            animations: AnimationSettings::default(),
            export_include_settings: true,
            export_include_library: false,
            folders_enabled: true,
            _legacy_tools_enabled: false,
            show_track_duration: true,
            show_queue_actions: true,
            show_queue_artist: true,
            cover_show_artist: true,
            cover_show_progress: true,
            cover_show_controls: true,
            blur_background: BlurBackground::default(),
            blur_intensity: BLUR_INTENSITY_DEFAULT,
            blur_interface_opacity: BLUR_INTERFACE_OPACITY_DEFAULT,
            queue_deduplication: false,
            tag_editor_enabled: true,
            albums_show_year: true,
            albums_show_genre: true,
            albums_artist_display: AlbumsArtistDisplay::default(),
            albums_layout: AlbumsLayout::default(),
            startup_tab: StartupTab::default(),
            window_preset: WindowPreset::default(),
            custom_window_width: default_custom_window_width(),
            custom_window_height: default_custom_window_height(),
            artists_grouping: music_library::ArtistGrouping::default(),
            lyrics_from_internet: false,
            lyrics_prefer_lrclib: false,
            remote_enabled: false,
            remote_port: pulse_remote::DEFAULT_PORT,
            _legacy_torrent_sources: None,
            _legacy_torrent_upload: None,
            _legacy_network_cache_gb: None,
            _legacy_subsonic_servers: None,
            _legacy_jellyfin_servers: None,
            _legacy_dlna_servers: None,
            discord_enabled: false,
            font_scale: FontScale::default(),
            lyrics_font_size: LYRICS_FONT_SIZE_DEFAULT,
            lyrics_karaoke_fill: true,
            lyrics_dim_inactive: true,
            onboarding_complete: false,
            sleep_timer: SleepTimerSettings::default(),
        }
    }
}

pub const SLEEP_TIMER_DURATIONS: [u32; 6] = [15, 30, 45, 60, 90, 120];
pub const SLEEP_TIMER_MAX_MIN: u32 = 12 * 60;
pub const SLEEP_TIMER_FADE_STEPS: [u32; 26] = [
    5, 10, 15, 20, 25, 30, 40, 50, 60, 75, 90, 120, 180, 240, 300, 360, 480, 600, 720, 900, 1200,
    1500, 1800, 2400, 3000, 3600,
];

pub fn sleep_timer_fade_step(secs: u32) -> usize {
    SLEEP_TIMER_FADE_STEPS
        .iter()
        .enumerate()
        .min_by_key(|(_, step)| step.abs_diff(secs))
        .map_or(0, |(ix, _)| ix)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SleepTimerSettings {
    pub fade_out: bool,
    pub fade_secs: u32,
    pub auto: bool,
    pub auto_from_min: u16,
    pub auto_until_min: u16,
    pub auto_duration_min: u32,
}

impl Default for SleepTimerSettings {
    fn default() -> Self {
        Self {
            fade_out: true,
            fade_secs: 30,
            auto: false,
            auto_from_min: 23 * 60,
            auto_until_min: 6 * 60,
            auto_duration_min: 30,
        }
    }
}

impl SleepTimerSettings {
    fn sanitized(mut self) -> Self {
        let default = Self::default();
        if self.auto_from_min >= 24 * 60 {
            self.auto_from_min = default.auto_from_min;
        }
        if self.auto_until_min >= 24 * 60 {
            self.auto_until_min = default.auto_until_min;
        }
        if self.auto_duration_min == 0 {
            self.auto_duration_min = default.auto_duration_min;
        }
        self.auto_duration_min = self.auto_duration_min.min(SLEEP_TIMER_MAX_MIN);
        self.fade_secs = SLEEP_TIMER_FADE_STEPS[sleep_timer_fade_step(self.fade_secs)];
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceState {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self { enabled: true }
    }
}

pub struct SettingsStore {
    pub settings: UserSettings,
    path: PathBuf,
    save_tx: Option<flume::Sender<SaveRequest>>,
}

impl Global for SettingsStore {}

/// How long a playback-only write waits for the next one before touching disk.
const PLAYBACK_SAVE_DELAY: Duration = Duration::from_millis(1500);

/// A settings write waiting for the background writer.
///
/// The distinction exists because the playback block changes constantly (queue
/// index, position) while everything else changes when a person clicks
/// something, and only the second kind deserves an immediate flush.
struct SaveRequest {
    settings: UserSettings,
    playback_only: bool,
}

impl SaveRequest {
    fn deferred(&self) -> bool {
        self.playback_only
    }

    fn settings(&self) -> &UserSettings {
        &self.settings
    }
}

fn write_settings(path: &Path, settings: &UserSettings) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Reads settings from `path`.
///
/// A missing file is the normal first-run case and yields defaults silently. A
/// file that exists but will not parse is a different failure: falling through
/// to defaults would quietly discard the library folders and playback state, and
/// the next save would overwrite the evidence. Set it aside instead so it stays
/// recoverable.
fn read_settings(path: &Path) -> UserSettings {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return UserSettings::default();
        }
        Err(e) => {
            log::error!(
                "settings: cannot read {} ({e}); starting from defaults",
                path.display()
            );
            return UserSettings::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(settings) => settings,
        Err(e) => {
            let kept = path.with_extension("json.unreadable");
            let _ = std::fs::remove_file(&kept);
            match std::fs::rename(path, &kept) {
                Ok(()) => log::error!(
                    "settings: {} is not valid JSON ({e}); kept it at {} and starting from defaults",
                    path.display(),
                    kept.display()
                ),
                Err(rename_err) => log::error!(
                    "settings: {} is not valid JSON ({e}) and could not be set aside ({rename_err}); \
                     it will be overwritten on the next save",
                    path.display()
                ),
            }
            UserSettings::default()
        }
    }
}

impl SettingsStore {
    pub fn load() -> Self {
        Self::load_from(Self::default_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let settings = read_settings(&path);
        Self {
            settings,
            path,
            save_tx: None,
        }
    }

    pub fn start_background_writer(&mut self, executor: BackgroundExecutor) {
        let (tx, rx) = flume::unbounded::<SaveRequest>();
        self.save_tx = Some(tx);
        let path = self.path.clone();
        executor
            .spawn(async move {
                while let Ok(mut request) = rx.recv_async().await {
                    // A playback write is only the queue index and a position:
                    // it changes on every track transition, so it waits for a
                    // quiet moment instead of hitting the disk each time. A
                    // debounced request is still written, just later; anything
                    // the user actually toggled goes straight through.
                    if request.deferred() {
                        let deadline = Instant::now() + PLAYBACK_SAVE_DELAY;
                        while let Ok(next) = rx.recv_deadline(deadline) {
                            let immediate = !next.deferred();
                            request = next;
                            if immediate {
                                break;
                            }
                        }
                    }
                    while let Ok(newer) = rx.try_recv() {
                        let immediate = !newer.deferred();
                        request = newer;
                        if immediate {
                            break;
                        }
                    }
                    if let Err(e) = write_settings(&path, request.settings()) {
                        log::error!("settings: background save failed: {e}");
                    }
                }
            })
            .detach();
    }

    fn default_path() -> PathBuf {
        crate::paths::config_dir()
            .unwrap_or_else(|| {
                log::warn!("settings: config dir unavailable, using current directory");
                PathBuf::from(".")
            })
            .join("settings.json")
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.enqueue(false)
    }

    fn enqueue(&self, playback_only: bool) -> anyhow::Result<()> {
        match &self.save_tx {
            Some(tx) => {
                let _ = tx.send(SaveRequest {
                    settings: self.settings.clone(),
                    playback_only,
                });
                Ok(())
            }
            None => write_settings(&self.path, &self.settings),
        }
    }

    /// Put every preference back to its factory value.
    ///
    /// Two things survive on purpose: `music_folders`, because without them the
    /// library would come up empty after a reset, and `onboarding_complete`,
    /// because a reset is not a first run. Nothing in the database is touched —
    /// playlists, play history and fetched lyrics all live there.
    pub fn reset_preferences(&mut self) -> anyhow::Result<()> {
        let folders = self.settings.music_folders.clone();
        let onboarding = self.settings.onboarding_complete;
        self.settings = UserSettings::default();
        self.settings.music_folders = folders;
        self.settings.onboarding_complete = onboarding;
        self.save()
    }

    /// Back to the default window preset and the default custom size, for a
    /// window that opened somewhere unusable.
    pub fn reset_window_size(&mut self) -> anyhow::Result<()> {
        self.settings.window_preset = WindowPreset::default();
        self.settings.custom_window_width = default_custom_window_width();
        self.settings.custom_window_height = default_custom_window_height();
        self.save()
    }

    pub fn save_playback_blocking(&mut self, playback: PlaybackState) -> anyhow::Result<()> {
        self.settings.playback = playback;
        self.save_tx = None;
        write_settings(&self.path, &self.settings)
    }

    pub fn dynamic_theme(&self) -> bool {
        self.settings.dynamic_theme
    }

    pub fn set_dynamic_theme(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.dynamic_theme = enabled;
        self.save()
    }

    pub fn music_folders(&self) -> &[PathBuf] {
        &self.settings.music_folders
    }

    pub fn add_music_folder(&mut self, path: PathBuf) -> anyhow::Result<()> {
        if self.settings.music_folders.iter().any(|p| p == &path) {
            return Ok(());
        }
        self.settings.music_folders.push(path);
        self.save()
    }

    pub fn remove_music_folder(&mut self, path: &Path) -> anyhow::Result<()> {
        let before = self.settings.music_folders.len();
        self.settings.music_folders.retain(|p| p.as_path() != path);
        if self.settings.music_folders.len() == before {
            return Ok(());
        }
        self.save()
    }

    pub fn playback(&self) -> &PlaybackState {
        &self.settings.playback
    }

    pub fn set_playback(&mut self, state: PlaybackState) -> anyhow::Result<()> {
        self.settings.playback = state;
        // Deferred: see `SaveRequest`. The quit path uses
        // `save_playback_blocking`, which writes synchronously.
        self.enqueue(true)
    }

    pub fn volume(&self) -> f32 {
        self.settings.volume
    }

    pub fn duplicate_priority(&self) -> &[String] {
        &self.settings.duplicate_priority
    }

    pub fn set_duplicate_priority(&mut self, priority: Vec<String>) -> anyhow::Result<()> {
        self.settings.duplicate_priority = priority;
        self.save()
    }

    pub fn replaygain(&self) -> ReplayGainMode {
        self.settings.replaygain
    }

    pub fn set_replaygain(&mut self, mode: ReplayGainMode) -> anyhow::Result<()> {
        self.settings.replaygain = mode;
        self.save()
    }

    pub fn replaygain_prevent_clipping(&self) -> bool {
        self.settings.replaygain_prevent_clipping
    }

    pub fn set_replaygain_prevent_clipping(&mut self, on: bool) -> anyhow::Result<()> {
        self.settings.replaygain_prevent_clipping = on;
        self.save()
    }

    pub fn set_volume(&mut self, volume: f32) -> anyhow::Result<()> {
        self.settings.volume = volume;
        self.save()
    }

    pub fn show_hog_button(&self) -> bool {
        self.settings.show_hog_button
    }

    pub fn set_show_hog_button(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_hog_button = show;
        self.save()
    }

    pub fn show_device_picker(&self) -> bool {
        self.settings.show_device_picker
    }

    pub fn set_show_device_picker(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_device_picker = show;
        self.save()
    }

    pub fn lyrics_from_internet(&self) -> bool {
        self.settings.lyrics_from_internet
    }

    pub fn set_lyrics_from_internet(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.lyrics_from_internet = enabled;
        self.save()
    }

    pub fn lyrics_prefer_lrclib(&self) -> bool {
        self.settings.lyrics_from_internet && self.settings.lyrics_prefer_lrclib
    }

    pub fn lyrics_prefer_lrclib_setting(&self) -> bool {
        self.settings.lyrics_prefer_lrclib
    }

    pub fn set_lyrics_prefer_lrclib(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.lyrics_prefer_lrclib = enabled;
        self.save()
    }

    pub fn remote_enabled(&self) -> bool {
        self.settings.remote_enabled
    }

    pub fn set_remote_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.remote_enabled = enabled;
        self.save()
    }

    pub fn remote_port(&self) -> u16 {
        self.settings.remote_port
    }

    pub fn set_remote_port(&mut self, port: u16) -> anyhow::Result<()> {
        self.settings.remote_port = port;
        self.save()
    }

    pub fn discord_enabled(&self) -> bool {
        self.settings.discord_enabled
    }

    pub fn set_discord_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.discord_enabled = enabled;
        self.save()
    }

    pub fn show_repeat_shuffle(&self) -> bool {
        self.settings.show_repeat_shuffle
    }

    pub fn set_show_repeat_shuffle(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_repeat_shuffle = show;
        self.save()
    }

    pub fn show_time_labels(&self) -> bool {
        self.settings.show_time_labels
    }

    pub fn set_show_time_labels(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_time_labels = show;
        self.save()
    }

    pub fn albums_enabled(&self) -> bool {
        self.settings.albums_enabled
    }

    pub fn set_albums_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.albums_enabled = enabled;
        self.save()
    }

    pub fn artists_enabled(&self) -> bool {
        self.settings.artists_enabled
    }

    pub fn set_artists_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.artists_enabled = enabled;
        self.save()
    }

    pub fn playlists_enabled(&self) -> bool {
        self.settings.playlists_enabled
    }

    pub fn set_playlists_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.playlists_enabled = enabled;
        self.save()
    }

    pub fn export_include_settings(&self) -> bool {
        self.settings.export_include_settings
    }

    pub fn export_include_library(&self) -> bool {
        self.settings.export_include_library
    }

    pub fn set_export_include_settings(&mut self, on: bool) -> anyhow::Result<()> {
        self.settings.export_include_settings = on;
        self.save()
    }

    pub fn set_export_include_library(&mut self, on: bool) -> anyhow::Result<()> {
        self.settings.export_include_library = on;
        self.save()
    }

    pub fn animations(&self) -> AnimationSettings {
        self.settings.animations
    }

    pub fn set_animations(&mut self, prefs: AnimationSettings) -> anyhow::Result<()> {
        self.settings.animations = prefs;
        self.save()
    }

    pub fn rewind_enabled(&self) -> bool {
        self.settings.rewind_enabled
    }

    pub fn set_rewind_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.rewind_enabled = enabled;
        self.save()
    }

    pub fn folders_enabled(&self) -> bool {
        self.settings.folders_enabled
    }

    pub fn set_folders_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.folders_enabled = enabled;
        self.save()
    }

    pub fn sleep_timer(&self) -> SleepTimerSettings {
        self.settings.sleep_timer.sanitized()
    }

    pub fn set_sleep_timer(&mut self, settings: SleepTimerSettings) -> anyhow::Result<()> {
        self.settings.sleep_timer = settings;
        self.save()
    }

    pub fn show_track_duration(&self) -> bool {
        self.settings.show_track_duration
    }

    pub fn set_show_track_duration(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_track_duration = show;
        self.save()
    }

    pub fn show_queue_actions(&self) -> bool {
        self.settings.show_queue_actions
    }

    pub fn set_show_queue_actions(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_queue_actions = show;
        self.save()
    }

    pub fn show_queue_artist(&self) -> bool {
        self.settings.show_queue_artist
    }

    pub fn set_show_queue_artist(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.show_queue_artist = show;
        self.save()
    }

    pub fn cover_show_artist(&self) -> bool {
        self.settings.cover_show_artist
    }

    pub fn set_cover_show_artist(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.cover_show_artist = show;
        self.save()
    }

    pub fn cover_show_progress(&self) -> bool {
        self.settings.cover_show_progress
    }

    pub fn set_cover_show_progress(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.cover_show_progress = show;
        self.save()
    }

    pub fn cover_show_controls(&self) -> bool {
        self.settings.cover_show_controls
    }

    pub fn set_cover_show_controls(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.cover_show_controls = show;
        self.save()
    }

    pub fn blur_background(&self) -> BlurBackground {
        self.settings.blur_background
    }

    pub fn set_blur_background(&mut self, mode: BlurBackground) -> anyhow::Result<()> {
        self.settings.blur_background = mode;
        self.save()
    }

    pub fn blur_intensity(&self) -> f32 {
        self.settings
            .blur_intensity
            .clamp(BLUR_INTENSITY_MIN, BLUR_INTENSITY_MAX)
    }

    pub fn set_blur_intensity(&mut self, value: f32) -> anyhow::Result<()> {
        self.settings.blur_intensity = value.clamp(BLUR_INTENSITY_MIN, BLUR_INTENSITY_MAX);
        self.save()
    }

    pub fn blur_interface_opacity(&self) -> f32 {
        self.settings
            .blur_interface_opacity
            .clamp(BLUR_INTERFACE_OPACITY_MIN, BLUR_INTERFACE_OPACITY_MAX)
    }

    pub fn set_blur_interface_opacity(&mut self, value: f32) -> anyhow::Result<()> {
        self.settings.blur_interface_opacity =
            value.clamp(BLUR_INTERFACE_OPACITY_MIN, BLUR_INTERFACE_OPACITY_MAX);
        self.save()
    }

    pub fn queue_deduplication(&self) -> bool {
        self.settings.queue_deduplication
    }

    pub fn set_queue_deduplication(&mut self, value: bool) -> anyhow::Result<()> {
        self.settings.queue_deduplication = value;
        self.save()
    }

    pub fn tag_editor_enabled(&self) -> bool {
        self.settings.tag_editor_enabled
    }

    pub fn set_tag_editor_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.tag_editor_enabled = enabled;
        self.save()
    }

    pub fn albums_show_year(&self) -> bool {
        self.settings.albums_show_year
    }

    pub fn set_albums_show_year(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.albums_show_year = show;
        self.save()
    }

    pub fn albums_show_genre(&self) -> bool {
        self.settings.albums_show_genre
    }

    pub fn set_albums_show_genre(&mut self, show: bool) -> anyhow::Result<()> {
        self.settings.albums_show_genre = show;
        self.save()
    }

    pub fn now_playing_details(&self) -> NowPlayingDetails {
        self.settings.now_playing_details
    }

    pub fn set_now_playing_details(&mut self, details: NowPlayingDetails) -> anyhow::Result<()> {
        self.settings.now_playing_details = details;
        self.save()
    }

    pub fn albums_artist_display(&self) -> AlbumsArtistDisplay {
        self.settings.albums_artist_display
    }

    pub fn set_albums_artist_display(
        &mut self,
        display: AlbumsArtistDisplay,
    ) -> anyhow::Result<()> {
        self.settings.albums_artist_display = display;
        self.save()
    }

    pub fn albums_layout(&self) -> AlbumsLayout {
        self.settings.albums_layout
    }

    pub fn set_albums_layout(&mut self, layout: AlbumsLayout) -> anyhow::Result<()> {
        self.settings.albums_layout = layout;
        self.save()
    }

    pub fn startup_tab(&self) -> StartupTab {
        match self.settings.startup_tab {
            // The Genres tab is gone. The variant survives only to parse an
            // older file, and is reported as Albums so the Settings dropdown
            // never shows a choice that does not exist.
            StartupTab::Genres => StartupTab::Albums,
            tab => tab,
        }
    }

    pub fn set_startup_tab(&mut self, tab: StartupTab) -> anyhow::Result<()> {
        self.settings.startup_tab = tab;
        self.save()
    }

    pub fn window_preset(&self) -> WindowPreset {
        self.settings.window_preset
    }

    pub fn set_window_preset(&mut self, preset: WindowPreset) -> anyhow::Result<()> {
        self.settings.window_preset = preset;
        self.save()
    }

    pub fn custom_window_size(&self) -> (f32, f32) {
        (
            self.settings.custom_window_width,
            self.settings.custom_window_height,
        )
    }

    pub fn set_custom_window_size(&mut self, width: f32, height: f32) -> anyhow::Result<()> {
        let w = sane_dimension(width, WINDOW_MIN_WIDTH);
        let h = sane_dimension(height, WINDOW_MIN_HEIGHT);
        self.settings.custom_window_width = w;
        self.settings.custom_window_height = h;
        self.save()
    }

    /// The size the main window should open at, for a monitor whose usable area
    /// is `monitor_width` x `monitor_height`.
    pub fn window_size_for_monitor(&self, monitor_width: f32, monitor_height: f32) -> (f32, f32) {
        resolve_window_size(
            self.settings.window_preset,
            self.settings.custom_window_width,
            self.settings.custom_window_height,
            monitor_width,
            monitor_height,
        )
    }

    pub fn artists_grouping(&self) -> music_library::ArtistGrouping {
        self.settings.artists_grouping
    }

    pub fn set_artists_grouping(
        &mut self,
        grouping: music_library::ArtistGrouping,
    ) -> anyhow::Result<()> {
        self.settings.artists_grouping = grouping;
        self.save()
    }

    pub fn font_scale(&self) -> FontScale {
        self.settings.font_scale
    }

    pub fn set_font_scale(&mut self, scale: FontScale) -> anyhow::Result<()> {
        self.settings.font_scale = scale;
        self.save()
    }

    pub fn lyrics_font_size(&self) -> f32 {
        self.settings
            .lyrics_font_size
            .clamp(LYRICS_FONT_SIZE_MIN, LYRICS_FONT_SIZE_MAX)
    }

    pub fn set_lyrics_font_size(&mut self, size: f32) -> anyhow::Result<()> {
        self.settings.lyrics_font_size = size.clamp(LYRICS_FONT_SIZE_MIN, LYRICS_FONT_SIZE_MAX);
        self.save()
    }

    pub fn lyrics_karaoke_fill(&self) -> bool {
        self.settings.lyrics_karaoke_fill
    }

    pub fn set_lyrics_karaoke_fill(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.lyrics_karaoke_fill = enabled;
        self.save()
    }

    pub fn lyrics_dim_inactive(&self) -> bool {
        self.settings.lyrics_dim_inactive
    }

    pub fn set_lyrics_dim_inactive(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.settings.lyrics_dim_inactive = enabled;
        self.save()
    }

    pub fn onboarding_complete(&self) -> bool {
        self.settings.onboarding_complete
    }

    pub fn set_onboarding_complete(&mut self, complete: bool) -> anyhow::Result<()> {
        self.settings.onboarding_complete = complete;
        self.save()
    }
}

/// Multiplier for chrome sized in raw pixels (bar heights, icon hitboxes,
/// cover art) so it tracks the [`FontScale`] the rem-relative text already follows.
pub fn ui_scale(cx: &App) -> f32 {
    cx.global::<SettingsStore>().font_scale().ui_scale()
}

/// Set the UI base font size (the rem unit) from a [`FontScale`]. `Root::render`
/// reapplies `cx.theme().font_size` as the window rem size every frame, so this
/// rescales every rem-relative text size across the app.
pub fn apply_font_scale(scale: FontScale, cx: &mut App) {
    Theme::global_mut(cx).font_size = scale.px();
}

fn reassert_font_scale(cx: &mut App) {
    if cx.has_global::<SettingsStore>() {
        apply_font_scale(cx.global::<SettingsStore>().font_scale(), cx);
    }
}

/// Apply Pulse's single built-in look: `Default Dark` from `gpui-component`.
/// This is the only theme the app ships; the cover-skin feature recolours it at
/// runtime and calls back here to restore the original palette.
pub fn apply_base_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::global_mut(cx).scrollbar_mode = ScrollbarMode::Scrolling;
    reassert_font_scale(cx);
}

pub fn apply_startup_theme(store: &SettingsStore, cx: &mut App) {
    apply_base_theme(cx);
    apply_font_scale(store.font_scale(), cx);
}

/// Push a user-visible notification (and log) when saving settings fails.
/// Looks up the active window via `cx.active_window()` — settings writes
/// always happen in response to UI actions, so a window is present.
pub fn notify_save_error(cx: &mut App, err: anyhow::Error) {
    log::error!("settings: save failed: {err}");
    if let Some(handle) = cx.active_window() {
        let _ = handle.update(cx, |_, window, cx| {
            let s = tr();
            window.push_notification(
                Notification::error(s.failed_save_settings(&err.to_string()))
                    .title(s.settings.clone()),
                cx,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn tmp_settings_path() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pulse-settings-test-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            // monotonically-ish unique per test invocation
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let path = tmp_settings_path();
        // Sanity: file doesn't exist yet.
        assert!(!path.exists());

        let store = SettingsStore::load_from(path.clone());
        assert!(store.music_folders().is_empty());

        cleanup(&path);
    }

    #[test]
    fn unparseable_settings_are_set_aside_instead_of_overwritten() {
        let path = tmp_settings_path();
        fs::write(&path, "{ this is not json").unwrap();

        let _store = SettingsStore::load_from(path.clone());

        // The unreadable file is preserved next to it, and the path itself is
        // free again so the next save can write a valid settings file.
        let kept = path.with_extension("json.unreadable");
        assert!(kept.exists(), "corrupt settings were not preserved");
        assert!(!path.exists(), "corrupt settings were left in place");

        // Loading again must be stable: the quarantined file is no longer at
        // `path`, so this takes the missing-file path and still yields defaults.
        let _reloaded = SettingsStore::load_from(path.clone());

        cleanup(&path);
    }

    #[test]
    fn load_returns_defaults_when_file_corrupt() {
        let path = tmp_settings_path();
        fs::write(&path, "{ this is not valid json").unwrap();

        let store = SettingsStore::load_from(path.clone());
        assert!(store.music_folders().is_empty());

        cleanup(&path);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let path = tmp_settings_path();
        let folder_a = PathBuf::from("/Users/test/Music");
        let folder_b = PathBuf::from("/Users/test/More Music");

        {
            let mut store = SettingsStore::load_from(path.clone());
            store.add_music_folder(folder_a.clone()).unwrap();
            store.add_music_folder(folder_b.clone()).unwrap();
        }

        let reloaded = SettingsStore::load_from(path.clone());
        assert_eq!(reloaded.music_folders(), &[folder_a, folder_b]);

        cleanup(&path);
    }

    #[test]
    fn add_music_folder_is_idempotent() {
        let path = tmp_settings_path();
        let folder = PathBuf::from("/Users/test/Music");

        let mut store = SettingsStore::load_from(path.clone());
        store.add_music_folder(folder.clone()).unwrap();
        store.add_music_folder(folder.clone()).unwrap();

        assert_eq!(store.music_folders(), &[folder]);

        cleanup(&path);
    }

    #[test]
    fn remove_music_folder_removes_matching_path() {
        let path = tmp_settings_path();
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        let c = PathBuf::from("/c");

        let mut store = SettingsStore::load_from(path.clone());
        store.add_music_folder(a.clone()).unwrap();
        store.add_music_folder(b.clone()).unwrap();
        store.add_music_folder(c.clone()).unwrap();

        store.remove_music_folder(&b).unwrap();
        assert_eq!(store.music_folders(), &[a, c]);

        // Removing a non-existent path is a no-op.
        store.remove_music_folder(Path::new("/missing")).unwrap();

        cleanup(&path);
    }

    #[test]
    fn save_uses_atomic_rename_via_tmp_file() {
        let path = tmp_settings_path();
        let store = {
            let mut s = SettingsStore::load_from(path.clone());
            s.add_music_folder(PathBuf::from("/music")).unwrap();
            s
        };

        // The tmp file should be gone after a successful save (renamed onto target).
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists());
        assert!(path.exists());
        // And the JSON should be readable / well-formed.
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"music_folders\""));
        assert!(raw.contains("/music"));

        drop(store);
        cleanup(&path);
    }

    #[test]
    fn removed_features_still_load_and_are_dropped_on_save() {
        // A settings.json written before the removals must keep loading: the
        // retired keys are accepted, ignored, and not written back out.
        // Shaped like a real settings.json from before the removals, down to
        // `torrent_upload` being the string the old UI wrote.
        let old = serde_json::json!({
            "network_cache_gb": 8,
            "tools_enabled": false,
            "torrent_sources": [],
            "torrent_upload": "off",
            "subsonic_servers": [{ "url": "http://nas", "username": "u", "password": "p" }],
            "jellyfin_servers": [{
                "url": "http://nas",
                "username": "u",
                "user_id": "1",
                "token": "t",
                "device_id": "d"
            }],
            "dlna_servers": [{ "udn": "uuid:x", "location": "http://nas/", "name": "NAS" }],
            "scrobble": {
                "lastfm": { "enabled": true, "session": null, "send_loves": true },
                "librefm": { "enabled": true, "session": null, "send_loves": true },
                "listenbrainz": { "enabled": false, "token": "t", "user": "u" },
                "csv_log": { "enabled": true, "path": "/tmp/scrobbles.csv" },
                "servers": { "server:1": { "plays": true, "likes": true } }
            },
        });
        let settings: UserSettings = serde_json::from_value(old).expect("old settings must load");
        let saved = serde_json::to_value(&settings).unwrap();
        for gone in [
            "network_cache_gb",
            "tools_enabled",
            "torrent_sources",
            "torrent_upload",
            "subsonic_servers",
            "jellyfin_servers",
            "dlna_servers",
            // Scrobbling was removed entirely in 4.0, so the whole section is
            // dropped rather than carried forward.
            "scrobble",
        ] {
            assert!(saved.get(gone).is_none(), "{gone} must not be saved again");
        }
        // And the values that are still there keep working.
        assert_eq!(settings.window_preset, WindowPreset::default());
        assert_eq!(settings.startup_tab, StartupTab::default());
    }

    #[test]
    fn window_presets_scale_with_the_monitor() {
        // 1080p with a panel: the default must land near the size the window is
        // actually run at, not somewhere else entirely.
        let (w, h) = resolve_window_size(WindowPreset::Landscape, 0., 0., 1920., 1032.);
        assert!((w - 1856.6).abs() < 2., "landscape width {w}");
        assert!((h - 701.8).abs() < 2., "landscape height {h}");

        // Same preset, a much smaller usable area: the same fractions of it, so
        // the window keeps its margins instead of a fixed pixel size.
        let (w, h) = resolve_window_size(WindowPreset::Landscape, 0., 0., 1280., 720.);
        assert!((w - 1237.76).abs() < 2., "landscape width {w}");
        assert!((h - 489.6).abs() < 2., "landscape height {h}");
        assert!(w < 1280. && h < 720., "must fit the usable area: {w}x{h}");

        // Portrait takes far more of the height than the landscape preset does.
        let (pw, ph) = resolve_window_size(WindowPreset::Portrait, 0., 0., 1920., 1032.);
        let (lw, lh) = resolve_window_size(WindowPreset::Landscape, 0., 0., 1920., 1032.);
        assert!(ph > lh, "portrait is taller: {pw}x{ph} vs {lw}x{lh}");
        assert!(pw < lw, "portrait is narrower: {pw}x{ph} vs {lw}x{lh}");
        assert!(pw >= WINDOW_MIN_WIDTH && ph <= 1032., "must fit: {pw}x{ph}");
        // Taller than wide where the minimum width leaves room for it.
        assert!(pw < ph, "portrait is taller than wide: {pw}x{ph}");
    }

    #[test]
    fn window_presets_are_clamped_to_the_monitor_and_minimums() {
        // Bigger than the screen: clipped to it.
        let (w, h) = resolve_window_size(WindowPreset::Custom, 4000., 3000., 1920., 1032.);
        assert_eq!((w, h), (1920., 1032.));

        // Smaller than the minimum: the minimum wins over the screen, because a
        // window that cannot be laid out is worse than one that overflows.
        let (w, h) = resolve_window_size(WindowPreset::Custom, 100., 100., 800., 600.);
        assert_eq!((w, h), (WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT));

        // Nonsense custom values never produce a zero-sized window.
        let (w, h) = resolve_window_size(WindowPreset::Custom, f32::NAN, -5., 1920., 1080.);
        assert!(w >= WINDOW_MIN_WIDTH && h >= WINDOW_MIN_HEIGHT);
    }

    #[test]
    fn custom_window_size_is_stored_sane() {
        assert_eq!(sane_dimension(1857.4, WINDOW_MIN_WIDTH), 1857.);
        assert_eq!(sane_dimension(10., WINDOW_MIN_WIDTH), WINDOW_MIN_WIDTH);
        assert_eq!(
            sane_dimension(f32::NAN, WINDOW_MIN_HEIGHT),
            WINDOW_MIN_HEIGHT
        );
        assert_eq!(sane_dimension(0., WINDOW_MIN_HEIGHT), WINDOW_MIN_HEIGHT);
    }

    #[test]
    fn save_returns_error_when_parent_path_is_a_file() {
        // Force a save failure by making the parent of `settings.json` a file,
        // not a directory — `create_dir_all` will reject it.
        let dir = std::env::temp_dir().join(format!(
            "pulse-settings-test-fail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        // `parent_as_file` is a regular file masquerading as a directory.
        let parent_as_file = dir.join("not_a_dir");
        fs::write(&parent_as_file, b"").unwrap();
        let path = parent_as_file.join("settings.json");

        let mut store = SettingsStore::load_from(path);
        let err = store.add_music_folder(PathBuf::from("/music"));
        assert!(err.is_err(), "expected save to fail when parent is a file");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn playback_state_roundtrip() {
        let entry = QueueRef {
            id: 1,
            path: "/music/song.flac".to_string(),
            start_offset_ms: 0,
        };
        let settings = UserSettings {
            rewind_enabled: true,
            albums_enabled: false,
            artists_enabled: true,
            animations: AnimationSettings::default(),
            export_include_settings: true,
            export_include_library: false,
            dynamic_theme: false,
            music_folders: vec![],
            duplicate_priority: vec!["/music/FLAC".into()],
            volume: 0.42,
            replaygain: ReplayGainMode::Track,
            replaygain_prevent_clipping: false,
            playback: PlaybackState {
                source_name: Some("Smoke Album".to_string()),
                queue: vec![entry.clone()],
                original_queue: Some(vec![entry]),
                current_index: Some(0),
                position_ms: 12_345,
                shuffle: true,
                repeat: RepeatModePersist::All,
                source: QueueSourcePersist::Playlist(7),
                custom: true,
            },
            show_hog_button: true,
            show_device_picker: true,
            show_repeat_shuffle: true,
            show_time_labels: true,
            now_playing_details: NowPlayingDetails::Album,
            playlists_enabled: true,
            folders_enabled: true,
            _legacy_tools_enabled: true,
            show_track_duration: true,
            show_queue_actions: true,
            show_queue_artist: true,
            cover_show_artist: true,
            cover_show_progress: true,
            cover_show_controls: true,
            blur_background: BlurBackground::default(),
            blur_intensity: 12.5,
            blur_interface_opacity: 140.,
            queue_deduplication: false,
            tag_editor_enabled: false,
            albums_show_year: true,
            albums_show_genre: true,
            albums_artist_display: AlbumsArtistDisplay::Column,
            albums_layout: AlbumsLayout::Grid,
            startup_tab: StartupTab::Folders,
            window_preset: WindowPreset::Custom,
            custom_window_width: 1400.,
            custom_window_height: 800.,
            artists_grouping: music_library::ArtistGrouping::TrackArtist,
            lyrics_from_internet: false,
            lyrics_prefer_lrclib: false,
            remote_enabled: false,
            remote_port: pulse_remote::DEFAULT_PORT,
            _legacy_torrent_sources: None,
            _legacy_torrent_upload: None,
            _legacy_network_cache_gb: None,
            _legacy_subsonic_servers: None,
            _legacy_jellyfin_servers: None,
            _legacy_dlna_servers: None,
            discord_enabled: false,
            font_scale: FontScale::Large,
            lyrics_font_size: 20.,
            lyrics_karaoke_fill: true,
            lyrics_dim_inactive: true,
            onboarding_complete: false,
            sleep_timer: SleepTimerSettings {
                fade_out: false,
                fade_secs: 75,
                auto: true,
                auto_from_min: 22 * 60 + 30,
                auto_until_min: 5 * 60,
                auto_duration_min: 45,
            },
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: UserSettings = serde_json::from_str(&json).unwrap();
        assert!((back.volume - 0.42).abs() < f32::EPSILON);
        assert_eq!(back.font_scale, FontScale::Large);
        assert_eq!(back.now_playing_details, NowPlayingDetails::Album);
        assert_eq!(back.albums_artist_display, AlbumsArtistDisplay::Column);
        assert_eq!(back.albums_layout, AlbumsLayout::Grid);
        assert_eq!(
            back.artists_grouping,
            music_library::ArtistGrouping::TrackArtist
        );
        assert!(back.folders_enabled);
        assert!(!back.albums_enabled);
        assert!(back.artists_enabled);
        assert_eq!(back.playback.queue.len(), 1);
        assert_eq!(
            back.playback.queue[0],
            QueueRef {
                id: 1,
                path: "/music/song.flac".to_string(),
                start_offset_ms: 0,
            }
        );
        assert_eq!(back.playback.current_index, Some(0));
        assert_eq!(back.playback.position_ms, 12_345);
        assert!(back.playback.shuffle);
        assert_eq!(back.playback.repeat, RepeatModePersist::All);
        assert_eq!(back.duplicate_priority, vec!["/music/FLAC".to_string()]);
        assert_eq!(back.replaygain, ReplayGainMode::Track);
        assert!(!back.replaygain_prevent_clipping);
        assert_eq!(back.playback.source, QueueSourcePersist::Playlist(7));
        assert_eq!(
            back.playback.source_name.as_deref(),
            Some("Smoke Album"),
            "the queue header's label has to survive a restart"
        );
        assert!(back.playback.custom);
        assert_eq!(back.sleep_timer, settings.sleep_timer);
    }

    #[test]
    fn sleep_timer_defaults_and_partial_json() {
        let settings: UserSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.sleep_timer, SleepTimerSettings::default());
        let settings: UserSettings =
            serde_json::from_str(r#"{"sleep_timer":{"auto":true,"auto_from_min":9999}}"#).unwrap();
        assert!(settings.sleep_timer.auto);
        assert!(settings.sleep_timer.fade_out);
        assert_eq!(settings.sleep_timer.fade_secs, 30);
        assert_eq!(
            settings.sleep_timer.sanitized().auto_from_min,
            SleepTimerSettings::default().auto_from_min
        );
    }

    #[rstest::rstest]
    #[case::below_range(0, 5)]
    #[case::on_a_step(240, 240)]
    #[case::between_steps(35, 30)]
    #[case::closer_to_upper(110, 120)]
    #[case::above_range(9999, 3600)]
    fn sleep_timer_fade_snaps_to_a_slider_step(#[case] stored: u32, #[case] expected: u32) {
        let settings = SleepTimerSettings {
            fade_secs: stored,
            ..SleepTimerSettings::default()
        };
        assert_eq!(settings.sanitized().fade_secs, expected);
    }

    #[test]
    fn sleep_timer_fade_steps_grow() {
        assert!(SLEEP_TIMER_FADE_STEPS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(SLEEP_TIMER_FADE_STEPS[0], 5);
        assert_eq!(
            SLEEP_TIMER_FADE_STEPS[SLEEP_TIMER_FADE_STEPS.len() - 1],
            3600
        );
        assert!(SLEEP_TIMER_FADE_STEPS.contains(&SleepTimerSettings::default().fade_secs));
    }

    #[test]
    fn sleep_timer_duration_is_clamped() {
        let settings: UserSettings =
            serde_json::from_str(r#"{"sleep_timer":{"auto_duration_min":100000}}"#).unwrap();
        assert_eq!(
            settings.sleep_timer.sanitized().auto_duration_min,
            SLEEP_TIMER_MAX_MIN
        );
    }

    #[test]
    fn queue_source_all_tracks_roundtrip() {
        use crate::playback_queue::QueueSource;
        let persisted: QueueSourcePersist = QueueSource::AllTracks.into();
        assert_eq!(persisted, QueueSourcePersist::AllTracks);
        let back: QueueSource = persisted.into();
        assert_eq!(back, QueueSource::AllTracks);
        let json = serde_json::to_string(&persisted).unwrap();
        let de: QueueSourcePersist = serde_json::from_str(&json).unwrap();
        assert_eq!(de, QueueSourcePersist::AllTracks);
    }

    #[test]
    fn old_settings_without_playback_field_gets_defaults() {
        let json = r#"{"theme":"system","music_folders":[]}"#;
        let settings: UserSettings = serde_json::from_str(json).unwrap();
        assert!((settings.volume - 1.0).abs() < f32::EPSILON);
        assert!(settings.playback.queue.is_empty());
        assert_eq!(settings.playback.current_index, None);
        assert_eq!(settings.playback.position_ms, 0);
        assert_eq!(settings.font_scale, FontScale::Small);
    }

    /// The factory look: a gentle blur and completely transparent chrome, so
    /// the backdrop is the interface. Both only matter once the blur is on.
    #[test]
    fn the_blur_defaults_show_the_backdrop_through_the_interface() {
        let settings = UserSettings::default();
        assert!((settings.blur_intensity - 6.0).abs() < f32::EPSILON);
        assert!((settings.blur_interface_opacity - 0.0).abs() < f32::EPSILON);
        assert!(matches!(settings.blur_background, BlurBackground::CoverView));
    }

    /// A reset is not a factory wipe: the folder list is what makes the library
    /// exist, and losing it would empty the app.
    #[test]
    fn resetting_preferences_keeps_the_folders_and_the_onboarding_flag() {
        let path = tmp_settings_path();
        let mut store = SettingsStore::load_from(path.clone());
        store.settings.music_folders = vec![PathBuf::from("/music")];
        store.settings.onboarding_complete = true;
        store.settings.volume = 0.1;
        store.settings.dynamic_theme = true;
        store.settings.albums_enabled = false;

        store.reset_preferences().unwrap();

        assert_eq!(store.settings.music_folders, vec![PathBuf::from("/music")]);
        assert!(store.settings.onboarding_complete);
        assert!((store.settings.volume - 1.0).abs() < f32::EPSILON);
        assert!(!store.settings.dynamic_theme);
        assert!(store.settings.albums_enabled);
        cleanup(&path);
    }

    #[test]
    fn a_fresh_install_keeps_the_defaults() {
        let path = tmp_settings_path();
        fs::write(&path, "{}").unwrap();

        let store = SettingsStore::load_from(path.clone());
        assert!(store.folders_enabled());
        assert!(store.playlists_enabled());
        assert!(store.rewind_enabled());
        assert!((store.volume() - 1.0).abs() < f32::EPSILON);
        assert_eq!(store.remote_port(), pulse_remote::DEFAULT_PORT);
        assert_eq!(store.startup_tab(), StartupTab::Albums);
        cleanup(&path);
    }

    /// 4.0 wrote a whole `Track` into every queue entry. Those files have to keep
    /// loading, and must land on the same resolution path as the new refs — which
    /// they do for free, because serde ignores the fields `QueueRef` does not
    /// name. This test is the guard on that.
    #[test]
    fn a_queue_entry_written_by_an_older_build_still_loads() {
        let json = r#"{
            "queue": [{
                "id": 42,
                "path": "/music/old.flac",
                "title": "Old Title",
                "album_id": 7,
                "track_number": 3,
                "disc_number": 1,
                "duration_ms": 240000,
                "year": 2020,
                "cover_art_id": 9,
                "start_offset_ms": 1500,
                "bitrate": 320,
                "is_cue": true,
                "available": true
            }],
            "current_index": 0,
            "position_ms": 1000
        }"#;
        let state: PlaybackState = serde_json::from_str(json).unwrap();

        assert_eq!(
            state.queue,
            vec![QueueRef {
                id: 42,
                path: "/music/old.flac".to_string(),
                start_offset_ms: 1500,
            }]
        );
        assert_eq!(state.current_index, Some(0));
        assert_eq!(state.position_ms, 1000);
    }

    /// The point of the ref: nothing that the catalog already holds is written
    /// back to disk. This is what took `settings.json` from ~1 MB to a few tens
    /// of KB for a 2,000-track queue.
    #[test]
    fn a_queue_ref_persists_no_catalog_data() {
        let state = PlaybackState {
            queue: vec![QueueRef {
                id: 1,
                path: "/a".into(),
                start_offset_ms: 0,
            }],
            ..PlaybackState::default()
        };

        let json = serde_json::to_string(&state).unwrap();

        assert!(json.contains("\"path\":\"/a\""));
        assert!(
            !json.contains("title") && !json.contains("duration_ms"),
            "catalog data must not be persisted: {json}"
        );
    }

    #[test]
    fn a_queue_ref_defaults_its_offset_to_zero() {
        let ref_: QueueRef = serde_json::from_str(r#"{"id":1,"path":"/a"}"#).unwrap();

        assert_eq!(ref_.start_offset_ms, 0);
    }
}
