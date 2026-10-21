//! Hardcoded, compile-time-checked UI strings.
//!
//! Pulse ships **English only**, so there is no language table indirection
//! left. Design (see `doc.md`):
//! - [`Strings`] is the schema: one `SharedString` field per UI string. Built
//!   with the `const fn` `SharedString::new_static`, so every entry is the
//!   zero-allocation `Borrowed(&'static str)` variant and `.clone()` on the
//!   render hot path is free.
//! - [`en::EN`] is the single table, built with the `lang!` macro. Because
//!   `lang!` expands to a full struct literal, the compiler errors if a key is
//!   missing or misspelled.
//! - [`strings`] hands back that one static; there is no runtime lookup.
//! - Interpolated strings store a `{}`-placeholder template in a field and are
//!   read through the accessor methods on `impl Strings`.

use gpui::SharedString;

mod en;
mod tools;

pub use tools::{ToolsStrings, tools_strings};

/// One immutable string per UI label. See the module docs.
pub struct Strings {
    // --- Transport / footer ---
    pub play: SharedString,
    pub pause: SharedString,
    pub next: SharedString,
    pub previous: SharedString,
    pub shuffle: SharedString,
    pub repeat_mode: SharedString,
    pub volume: SharedString,
    pub mute: SharedString,
    pub unmute: SharedString,
    pub queue: SharedString,
    pub remove_from_queue: SharedString,
    pub queue_is_empty: SharedString,
    pub playing_from_t: SharedString,
    pub selection_count_t: SharedString,
    pub clear_selection: SharedString,

    // --- Duplicates ---
    pub duplicates_group: SharedString,
    pub duplicates_priority: SharedString,
    pub duplicates_desc: SharedString,
    pub duplicates_empty: SharedString,
    pub duplicates_add_folder: SharedString,
    pub duplicates_seed: SharedString,
    pub duplicates_find: SharedString,
    pub duplicates_found: SharedString,
    pub duplicates_none: SharedString,
    pub duplicates_confirm_title: SharedString,
    pub duplicates_confirm_body: SharedString,
    pub duplicates_move_to_trash: SharedString,
    pub duplicates_moved_t: SharedString,
    pub duplicates_failed_t: SharedString,
    pub duplicates_more_t: SharedString,
    pub duplicates_up: SharedString,
    pub duplicates_down: SharedString,
    pub playback_group: SharedString,
    pub replaygain: SharedString,
    pub replaygain_desc: SharedString,
    pub replaygain_off: SharedString,
    pub replaygain_track: SharedString,
    pub replaygain_album: SharedString,
    pub replaygain_clip: SharedString,
    pub replaygain_clip_desc: SharedString,
    pub clear_queue: SharedString,
    pub save_queue_to_playlist: SharedString,

    // --- Track-row actions ---
    pub add_to_playlist: SharedString,
    pub remove_from_playlist: SharedString,
    pub add_to_queue: SharedString,
    pub add_album_to_queue: SharedString,
    pub add_artist_tracks_to_queue: SharedString,
    pub replace_queue: SharedString,
    pub replace_queue_confirm_title: SharedString,
    pub replace_queue_confirm_message: SharedString,

    // --- Header / navigation ---
    pub settings: SharedString,
    pub search_placeholder: SharedString,
    pub tab_albums: SharedString,
    pub tab_artists: SharedString,
    pub tab_folders: SharedString,
    pub tab_playlists: SharedString,
    pub cover_mode: SharedString,
    pub full_screen: SharedString,
    pub all_tracks: SharedString,
    pub full_albums_tooltip: SharedString,
    pub no_metadata: SharedString,

    // --- Library empty / loading states ---
    pub scanning: SharedString,
    pub library_updating: SharedString,
    pub library_updated: SharedString,
    pub library_up_to_date: SharedString,
    pub library_update_failed: SharedString,
    pub library_folder_unavailable_t: SharedString,
    pub playback_failed_title: SharedString,
    pub local_folders: SharedString,
    pub decimal_separator: SharedString,
    pub size_mb_t: SharedString,
    pub size_gb_t: SharedString,
    pub source_online: SharedString,
    pub source_offline: SharedString,
    pub source_scanning: SharedString,
    pub unavailable_count_t: SharedString,
    pub no_music_sources: SharedString,
    pub open_library_settings: SharedString,
    pub no_albums_found: SharedString,
    pub no_albums_match: SharedString,
    pub no_artists_found: SharedString,
    pub no_artists_match: SharedString,
    pub no_tracks_for_album: SharedString,
    pub no_tracks_match: SharedString,
    pub no_tracks_for_artist: SharedString,
    pub no_folders_found: SharedString,
    pub no_folders_match: SharedString,
    pub no_tracks_in_folder: SharedString,
    pub tracks_in_this_folder: SharedString,
    pub playlist_is_empty: SharedString,

    // --- Playlists ---
    pub filter_playlists: SharedString,
    pub playlist_name: SharedString,
    pub name_new_playlist: SharedString,
    pub create: SharedString,
    pub cancel: SharedString,
    pub no_playlists_yet: SharedString,
    pub playlists_empty_hint: SharedString,
    pub no_playlists_match: SharedString,
    pub create_new_playlist: SharedString,
    pub new_playlist_name: SharedString,
    pub new_playlist: SharedString,
    pub delete: SharedString,
    pub rename: SharedString,
    pub tab_rewind: SharedString,
    pub rewind_desc: SharedString,

    // --- Settings page ---
    pub settings_appearance: SharedString,
    pub settings_general: SharedString,
    pub settings_online: SharedString,
    pub settings_lyrics_online: SharedString,
    pub settings_library: SharedString,
    pub dynamic_theme: SharedString,
    pub dynamic_theme_desc: SharedString,
    pub native_rate_button: SharedString,
    pub native_rate_button_desc: SharedString,
    pub device_picker_button: SharedString,
    pub device_picker_button_desc: SharedString,
    pub repeat_shuffle: SharedString,
    pub repeat_shuffle_desc: SharedString,
    pub time_labels: SharedString,
    pub time_labels_desc: SharedString,
    pub settings_now_playing: SharedString,
    pub now_playing_details: SharedString,
    pub now_playing_details_desc: SharedString,
    pub now_playing_details_specs: SharedString,
    pub now_playing_details_album: SharedString,
    pub font_size: SharedString,
    pub font_size_desc: SharedString,
    pub font_size_small: SharedString,
    pub font_size_medium: SharedString,
    pub font_size_large: SharedString,
    pub blur_background: SharedString,
    pub blur_background_desc: SharedString,
    pub blur_background_off: SharedString,
    pub blur_background_cover: SharedString,
    pub blur_background_all: SharedString,
    pub blur_intensity: SharedString,
    pub blur_intensity_desc: SharedString,
    pub blur_interface_opacity: SharedString,
    pub blur_interface_opacity_desc: SharedString,
    pub playlists_desc: SharedString,
    pub folders_desc: SharedString,
    pub albums_desc: SharedString,
    pub artists_desc: SharedString,
    pub tabs_all_tracks_note: SharedString,

    // --- Advanced / maintenance ---
    pub settings_advanced: SharedString,
    pub advanced_files: SharedString,
    pub advanced_maintenance: SharedString,
    pub advanced_danger: SharedString,
    pub advanced_data_dir: SharedString,
    pub advanced_config_dir: SharedString,
    pub advanced_logs_dir: SharedString,
    pub advanced_clear_covers: SharedString,
    pub advanced_clear_covers_desc: SharedString,
    pub advanced_clear_covers_done_t: SharedString,
    pub advanced_rescan: SharedString,
    pub advanced_rescan_desc: SharedString,
    pub advanced_rescan_started: SharedString,
    pub advanced_reset_window: SharedString,
    pub advanced_reset_window_desc: SharedString,
    pub advanced_reset_window_done: SharedString,
    pub advanced_vacuum: SharedString,
    pub advanced_vacuum_desc: SharedString,
    pub advanced_vacuum_done: SharedString,
    pub advanced_reset_all: SharedString,
    pub advanced_reset_all_desc: SharedString,
    pub advanced_reset_all_confirm: SharedString,
    pub advanced_reset_all_body: SharedString,
    pub advanced_reset_all_done: SharedString,
    pub not_available: SharedString,
    pub library_open_failed_title: SharedString,
    pub library_open_failed_body: SharedString,
    pub library_open_failed_hint: SharedString,
    pub library_open_failed_database: SharedString,
    pub library_open_failed_backups: SharedString,
    pub track_duration: SharedString,
    pub track_duration_desc: SharedString,
    pub action_buttons: SharedString,
    pub action_buttons_desc: SharedString,
    pub artist_name: SharedString,
    pub artist_name_desc: SharedString,
    pub cover_artist_desc: SharedString,
    pub cover_progress: SharedString,
    pub cover_progress_desc: SharedString,
    pub cover_controls: SharedString,
    pub cover_controls_desc: SharedString,
    pub queue_deduplication: SharedString,
    pub queue_deduplication_desc: SharedString,

    pub window_group: SharedString,
    pub startup_tab: SharedString,
    pub startup_tab_desc: SharedString,
    pub window_size: SharedString,
    pub window_size_desc: SharedString,
    pub window_size_landscape: SharedString,
    pub window_size_portrait: SharedString,
    pub window_size_custom: SharedString,
    pub window_width: SharedString,
    pub window_height: SharedString,
    pub albums_layout: SharedString,
    pub albums_layout_desc: SharedString,
    pub albums_layout_list: SharedString,
    pub albums_layout_grid: SharedString,
    pub year_column: SharedString,
    pub year_column_desc: SharedString,
    pub album_year: SharedString,
    pub album_year_desc: SharedString,
    pub genre_column: SharedString,
    pub genre_column_desc: SharedString,
    pub albums_artist_desc: SharedString,
    pub albums_artist_inline: SharedString,
    pub albums_artist_column: SharedString,
    pub albums_artist_hidden: SharedString,
    pub artists_group_by_tag: SharedString,
    pub no_folders_added: SharedString,
    pub reveal_folder: SharedString,
    pub remove: SharedString,
    pub add_folder: SharedString,
    pub remove_folder_confirm_title: SharedString,

    // --- Audio settings ---
    pub audio_device: SharedString,
    pub bit_perfect_playback: SharedString,
    pub not_bit_perfect: SharedString,
    pub bp_system_muted: SharedString,
    pub bp_no_source: SharedString,
    pub select_audio_device: SharedString,
    pub native_rate_title: SharedString,
    pub native_rate_click_disable: SharedString,
    pub native_rate_click_enable: SharedString,
    pub bp_native_rate_off: SharedString,
    pub default_suffix: SharedString,

    pub onboarding_title: SharedString,
    pub onboarding_subtitle: SharedString,
    pub onboarding_folder_prompt: SharedString,
    pub onboarding_finish: SharedString,
    pub pipewire_alsa_missing_title: SharedString,
    pub pipewire_alsa_missing_body: SharedString,
    pub copy: SharedString,

    // --- App menu ---
    pub quit_pulse: SharedString,
    pub rescan_library: SharedString,
    pub version: SharedString,
    pub up_to_date: SharedString,

    // --- Lyrics ---
    pub lyrics: SharedString,
    pub lyrics_empty: SharedString,
    pub lyrics_fetching: SharedString,
    pub lyrics_not_found: SharedString,
    pub lyrics_save: SharedString,
    pub lyrics_follow: SharedString,
    pub lyrics_from_internet: SharedString,
    pub lyrics_from_internet_desc: SharedString,
    pub lyrics_source_lrc: SharedString,
    pub lyrics_source_tags: SharedString,
    pub lyrics_source_lrclib: SharedString,
    pub lyrics_search_lrclib: SharedString,
    pub lyrics_searching_lrclib: SharedString,
    pub lyrics_prefer_lrclib: SharedString,
    pub lyrics_prefer_lrclib_desc: SharedString,
    pub remote_control: SharedString,
    pub remote_control_desc: SharedString,
    pub remote_port: SharedString,
    pub remote_port_desc: SharedString,
    pub open_in_browser: SharedString,
    pub remote_address: SharedString,
    pub remote_address_desc: SharedString,
    pub play_next: SharedString,
    pub playing_next_t: SharedString,
    pub backups: SharedString,
    pub restore_backup: SharedString,
    pub restore_backup_desc: SharedString,
    pub library_check: SharedString,
    pub library_check_desc: SharedString,
    pub check_now: SharedString,
    pub restore_scheduled: SharedString,
    pub library_check_clean: SharedString,
    pub settings_interface: SharedString,
    pub settings_library_pages: SharedString,
    pub discord_share: SharedString,
    pub discord_share_desc: SharedString,
    pub lyrics_text_size: SharedString,
    pub lyrics_text_size_desc: SharedString,
    pub lyrics_karaoke: SharedString,
    pub lyrics_karaoke_desc: SharedString,
    pub lyrics_dim_inactive: SharedString,
    pub lyrics_dim_inactive_desc: SharedString,

    // --- Tag editor ---
    pub edit_tags: SharedString,
    pub edit_album_tags: SharedString,
    pub tag_editor: SharedString,
    pub tag_editor_desc: SharedString,
    pub tag_save: SharedString,
    pub tag_title: SharedString,
    pub tag_artists: SharedString,
    pub tag_album: SharedString,
    pub tag_album_artists: SharedString,
    pub tag_track_number: SharedString,
    pub tag_disc_number: SharedString,
    pub tag_year: SharedString,
    pub tag_genres: SharedString,
    pub tag_add_genre: SharedString,
    pub tag_all_tags: SharedString,
    pub tag_custom_key: SharedString,
    pub tag_custom_value: SharedString,
    pub tag_custom_invalid: SharedString,
    pub tag_custom_invalid_len: SharedString,
    pub tag_cover: SharedString,
    pub tag_cover_replace: SharedString,
    pub tag_cover_remove: SharedString,
    pub tag_cover_will_remove: SharedString,
    pub tag_cover_album_hint: SharedString,
    pub tag_cover_failed_title: SharedString,
    pub tag_cover_unreadable: SharedString,
    pub tag_cover_bad_format: SharedString,
    pub tag_cue_readonly: SharedString,
    pub tag_playing_readonly: SharedString,
    pub tag_album_playing_readonly: SharedString,
    pub tag_album_fields_hint: SharedString,
    pub tags_saved: SharedString,
    pub tag_write_failed_title: SharedString,

    // --- Animations page ---
    pub settings_animations: SharedString,
    pub motion_group: SharedString,
    pub animations_play: SharedString,
    pub animations_play_desc: SharedString,
    pub animation_panel_slide: SharedString,
    pub animation_panel_slide_desc: SharedString,
    pub animation_view_fade: SharedString,
    pub animation_view_fade_desc: SharedString,
    pub animation_track_change: SharedString,
    pub animation_track_change_desc: SharedString,
    pub animation_cover_mode: SharedString,
    pub animation_cover_mode_desc: SharedString,
    pub animation_settings_open: SharedString,
    pub animation_settings_open_desc: SharedString,
    pub animation_cover_fade: SharedString,
    pub animation_cover_fade_desc: SharedString,

    // --- Version, Online and the portable copy ---
    pub made_by: SharedString,
    pub discord_group: SharedString,
    pub portable_copy: SharedString,
    pub portable_desc: SharedString,
    pub portable_include_settings: SharedString,
    pub portable_include_settings_desc: SharedString,
    pub portable_include_library: SharedString,
    pub portable_include_library_desc: SharedString,
    pub portable_export: SharedString,
    pub portable_export_button: SharedString,
    pub portable_choose_folder: SharedString,

    // --- Templated (read via the accessor methods below) ---
    pub tags_save_failed_t: SharedString,
    pub disc_t: SharedString,
    pub n_tracks_one: SharedString,
    pub n_tracks_other: SharedString,
    pub bp_system_volume_t: SharedString,
    pub bp_sample_rate_t: SharedString,
    pub bp_bit_depth_t: SharedString,
    pub failed_exclusive_t: SharedString,
    pub native_rate_failed_t: SharedString,
    pub failed_switch_device_t: SharedString,
    pub failed_save_settings_t: SharedString,
    pub remote_start_failed_t: SharedString,
    pub portable_exported_t: SharedString,
    pub portable_failed_t: SharedString,
}

/// Replace each `{}` marker in `template` with the next arg, in order.
fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut parts = template.split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for (arg, part) in args.iter().zip(parts) {
        out.push_str(arg);
        out.push_str(part);
    }
    out
}

impl Strings {
    pub fn duplicates_moved(&self, n: usize) -> String {
        fill(&self.duplicates_moved_t, &[&n.to_string()])
    }

    pub fn duplicates_failed(&self, n: usize) -> String {
        fill(&self.duplicates_failed_t, &[&n.to_string()])
    }

    pub fn duplicates_more(&self, n: usize) -> String {
        fill(&self.duplicates_more_t, &[&n.to_string()])
    }

    pub fn selection_count(&self, n: usize) -> String {
        fill(&self.selection_count_t, &[&n.to_string()])
    }

    pub fn playing_from(&self, source: &str) -> String {
        fill(&self.playing_from_t, &[source])
    }

    pub fn advanced_clear_covers_done(&self, covers: usize, files: usize) -> String {
        fill(
            &self.advanced_clear_covers_done_t,
            &[&covers.to_string(), &files.to_string()],
        )
    }

    pub fn portable_exported(&self, path: &str) -> String {
        fill(&self.portable_exported_t, &[path])
    }

    pub fn portable_failed(&self, err: &str) -> String {
        fill(&self.portable_failed_t, &[err])
    }

    /// Section header for a multi-disc album, e.g. "Disc 2".
    pub fn disc(&self, disc: u32) -> String {
        fill(&self.disc_t, &[&disc.to_string()])
    }

    /// Track-count label, e.g. "1 track" / "2 трека" / "5 треков". Picks the
    /// `one`/`few`/`other` form per this language's CLDR plural family.
    pub fn n_tracks(&self, n: i64) -> String {
        let template = if n == 1 {
            &self.n_tracks_one
        } else {
            &self.n_tracks_other
        };
        fill(template, &[&n.to_string()])
    }

    pub fn bp_system_volume(&self, value: &str) -> String {
        fill(&self.bp_system_volume_t, &[value])
    }

    pub fn bp_sample_rate(&self, source_hz: u32, device_hz: u32) -> String {
        fill(
            &self.bp_sample_rate_t,
            &[&source_hz.to_string(), &device_hz.to_string()],
        )
    }

    pub fn bp_bit_depth(&self, source: u32) -> String {
        fill(&self.bp_bit_depth_t, &[&source.to_string()])
    }

    pub fn failed_exclusive(&self, err: &str) -> String {
        fill(&self.failed_exclusive_t, &[err])
    }

    pub fn failed_native_rate(&self, err: &str) -> String {
        fill(&self.native_rate_failed_t, &[err])
    }

    pub fn failed_switch_device(&self, err: &str) -> String {
        fill(&self.failed_switch_device_t, &[err])
    }

    pub fn failed_save_settings(&self, err: &str) -> String {
        fill(&self.failed_save_settings_t, &[err])
    }

    pub fn remote_start_failed(&self, port: u16, err: &str) -> String {
        fill(&self.remote_start_failed_t, &[&port.to_string(), err])
    }

    pub fn library_folder_unavailable(&self, folder: &str) -> String {        fill(&self.library_folder_unavailable_t, &[folder])
    }

    pub fn size(&self, bytes: u64) -> String {
        const MB: f64 = 1024. * 1024.;
        let mb = bytes as f64 / MB;
        if mb.round() >= 1024. {
            let gb = mb / 1024.;
            let text = if (gb - gb.round()).abs() < 0.05 {
                format!("{}", gb.round() as u64)
            } else {
                format!("{gb:.1}").replace('.', &self.decimal_separator)
            };
            fill(&self.size_gb_t, &[&text])
        } else {
            fill(&self.size_mb_t, &[&format!("{}", mb.round() as u64)])
        }
    }

    pub fn unavailable_count(&self, count: usize) -> String {
        fill(&self.unavailable_count_t, &[&count.to_string()])
    }

    pub fn tags_save_failed(&self, err: &str) -> String {
        fill(&self.tags_save_failed_t, &[err])
    }

    pub fn playing_next(&self, title: &str) -> String {
        fill(&self.playing_next_t, &[title])
    }
}

/// Build a `Strings` literal from `key: "value"` pairs. Expands to a full struct
/// literal, so a missing or misspelled key is a compile error.
#[macro_export]
macro_rules! lang {
    ( $($key:ident : $val:literal),* $(,)? ) => {
        $crate::i18n::Strings {
            $( $key: ::gpui::SharedString::new_static($val) ),*
        }
    };
}

/// The string table. English is the only language, so this is a plain static.
pub fn strings() -> &'static Strings {
    &en::EN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_handles_placeholders() {
        assert_eq!(fill("Disc {}", &["2"]), "Disc 2");
        assert_eq!(
            fill("{} kHz · {}-bit", &["44.1", "24"]),
            "44.1 kHz · 24-bit"
        );
        assert_eq!(fill("no placeholders", &[]), "no placeholders");
    }

    #[test]
    fn size_uses_megabytes_below_a_gigabyte() {
        let tr = &super::en::EN;
        assert_eq!(tr.size(0), "0 MB");
        assert_eq!(tr.size(300 * 1024 * 1024), "300 MB");
        assert_eq!(tr.size(4 * 1024 * 1024 * 1024), "4 GB");
        assert_eq!(tr.size(1536 * 1024 * 1024), "1.5 GB");
        assert_eq!(tr.size(1024 * 1024 * 1024 - 1024), "1 GB");
    }
}
