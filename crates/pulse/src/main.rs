
use audio_output::AudioOutput;
use gpui::*;
use gpui_component::*;

use crate::{
    main_view::MainView,
    services::{Services, run_engine_events_bus},
};

pub mod app_menu;
pub mod audio_settings;
pub mod animations;
mod paths;
mod portable;
mod command_palette;
pub mod cover_art_cache;
pub mod cover_backdrop;
pub mod cover_mode_view;
pub mod cover_skin;
pub mod cover_volume;
pub mod discord_bridge;
pub mod duplicates;
pub mod error_bridge;
pub mod footer;
pub mod keyboard_shortcuts;
pub mod legacy_migration;
pub mod library_gate;
pub mod library_scan_indicator;
pub mod library_service;
pub mod library_sources;
pub mod library_views;
pub mod library_watcher;
pub mod localization;
pub mod lyrics_fill;
pub mod lyrics_view;
pub mod main_view;
pub mod media_bridge;
pub mod net;
pub mod next_button;
pub mod now_playing;
pub mod onboarding_view;
pub mod panel_header;
pub mod pipewire_alsa_gate;
pub mod play_button;
pub mod play_history;
pub mod playback_opener;
pub mod playback_queue;
pub mod playback_status;
pub mod playlist_popup;
pub mod prev_button;
pub mod queue_view;
pub mod repeat_button;
pub mod services;
pub mod settings_store;
pub mod settings_view;
pub mod shuffle_button;
pub mod single_instance;
pub mod sleep_timer;
pub mod tag_editor_view;
pub mod theme_colors;
// Tools screen removed: AI prompt, Covers and the standalone Timer page.
pub mod track_list;
pub mod track_progress_slider;
pub mod volume;
pub mod window_title_bar;

fn restore_engine_state(cx: &mut App) {
    let stored_position_ms = cx
        .global::<crate::settings_store::SettingsStore>()
        .playback()
        .position_ms;
    let services = cx.global::<Services>();
    let queue = services.playback_queue.borrow();
    let Some(track) = queue.current_track().cloned() else {
        return;
    };
    drop(queue);

    // Only restore a track that is actually on disk. A file can be missing
    // because a drive is unmounted; in that case keep the stored position so a
    // later start resumes where the user left off.
    let path = std::path::PathBuf::from(&track.path);
    if !path.exists() {
        if stored_position_ms > 0 {
            services.resume_at.set(Some((track.id, stored_position_ms)));
            services
                .current_position_ms
                .store(stored_position_ms, std::sync::atomic::Ordering::Relaxed);
        }
        return;
    }
    let start_offset = if track.start_offset_ms > 0 {
        Some(std::time::Duration::from_millis(
            track.start_offset_ms as u64,
        ))
    } else {
        None
    };
    let duration = track
        .duration_ms
        .map(|ms| std::time::Duration::from_millis(ms as u64));
    services
        .engine_manager
        .set_track_gain(crate::services::track_gain_for(services, &track, cx));
    services
        .engine_manager
        .set_track_with_offset(path, start_offset, duration);
    if stored_position_ms > 0
        && let Some(dur_ms) = track.duration_ms
        && dur_ms > 0
    {
        let ratio = (stored_position_ms as f32 / dur_ms as f32).clamp(0.0, 1.0);
        services
            .current_position_ms
            .store(stored_position_ms, std::sync::atomic::Ordering::Relaxed);
        services.engine_manager.seek(ratio);
    }
}

/// Usable size of the monitor the window will open on, in logical pixels.
///
/// Prefers the primary display and falls back to a conservative 1920x1080 so
/// window sizing never fails on a headless or Wayland-misreported setup.
fn usable_monitor_size(cx: &App) -> (f32, f32) {
    const FALLBACK: (f32, f32) = (1920., 1080.);
    let Some(display) = cx.primary_display() else {
        return FALLBACK;
    };
    let bounds = display.visible_bounds();
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();
    if w > 0. && h > 0. { (w, h) } else { FALLBACK }
}

/// The size the main window should open at.
///
/// The PipeWire/ALSA gate window runs before `SettingsStore` is installed as a
/// global, so that path falls back to the preset defaults rather than reading
/// user settings that are not loaded yet.
fn window_size_for(cx: &App) -> (f32, f32) {
    let (monitor_w, monitor_h) = usable_monitor_size(cx);
    match cx.try_global::<crate::settings_store::SettingsStore>() {
        Some(store) => store.window_size_for_monitor(monitor_w, monitor_h),
        None => crate::settings_store::resolve_window_size(
            crate::settings_store::WindowPreset::default(),
            0.,
            0.,
            monitor_w,
            monitor_h,
        ),
    }
}

pub(crate) fn build_window_options(cx: &mut App) -> WindowOptions {
    let (width, height) = window_size_for(cx);
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(
            px(crate::settings_store::WINDOW_MIN_WIDTH),
            px(crate::settings_store::WINDOW_MIN_HEIGHT),
        )),
        titlebar: Some(TitlebarOptions {
            title: Some("Pulse".into()),
            ..TitleBar::title_bar_options()
        }),
        app_id: Some("pulse".into()),
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

fn open_main_window(cx: &mut App, run_startup_tasks: bool) {
    let options = build_window_options(cx);
    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| MainView::new(window, cx));
        let root = cx.new(|cx| Root::new(view, window, cx));
        if run_startup_tasks {
            restore_engine_state(cx);
            window.on_next_frame(|_window, cx| {
                let folders = cx
                    .global::<crate::settings_store::SettingsStore>()
                    .music_folders()
                    .to_vec();
                if !folders.is_empty() {
                    cx.global::<Services>().library.clear_and_rescan(folders);
                }
                crate::library_watcher::rebuild(cx);
            });
        }
        root
    })
    .expect("Failed to open window");
}

pub fn open_onboarding_window(cx: &mut App) {
    let options = build_window_options(cx);
    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| crate::onboarding_view::OnboardingView::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("Failed to open onboarding window");
}

fn open_initial_window(cx: &mut App, run_startup_tasks: bool) {
    if cx
        .global::<crate::settings_store::SettingsStore>()
        .onboarding_complete()
    {
        open_main_window(cx, run_startup_tasks);
    } else {
        open_onboarding_window(cx);
    }
}

fn main() {
    // Before anything reads settings or the library: bring a Pawse 2.x install
    // forward to the Pulse directory names.
    legacy_migration::migrate_legacy_dirs();

    let single_instance = match single_instance::acquire() {
        single_instance::Acquire::Duplicate => return,
        single_instance::Acquire::First(listener) => listener,
    };

    let app = Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(ui_resources::assets::Assets);

    app.run(move |cx| {
        let log_dir = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("pulse-music")
            .join("logs");
        let notices = diagnostics::init(diagnostics::Config {
            log_dir,
            ..Default::default()
        });
        if let Err(err) = rlimit::increase_nofile_limit(u64::MAX) {
            log::warn!("could not raise open file limit: {err}");
        }

        gpui_component::init(cx);
        crate::playlist_popup::init(cx);
        crate::command_palette::init(cx);
        crate::keyboard_shortcuts::init(cx);
        crate::error_bridge::spawn_notice_forwarder(cx, notices);

        let settings_store = crate::settings_store::SettingsStore::load();
        crate::settings_store::apply_startup_theme(&settings_store, cx);
        cx.set_global(settings_store);
        let save_executor = cx.background_executor().clone();
        cx.global_mut::<crate::settings_store::SettingsStore>()
            .start_background_writer(save_executor);

        if crate::pipewire_alsa_gate::should_block_startup() {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
            crate::pipewire_alsa_gate::open(cx);
            single_instance::install(cx, single_instance);
            return;
        }

        let services = match Services::initialize(cx) {
            Ok(services) => services,
            Err(err) => {
                // No library, no app: show why instead of dying silently. The
                // gate is the same shape as the PipeWire one, and quitting is
                // the only action that makes sense from here.
                log::error!("library: could not open the database: {err}");
                cx.on_window_closed(|cx, _| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
                cx.activate(true);
                crate::library_gate::open(cx, format!("{err}"));
                single_instance::install(cx, single_instance);
                return;
            }
        };

        let engine_manager = services.engine_manager.clone();
        let engine_event_bus = services.engine_event_bus.clone();
        let current_position_ms = services.current_position_ms.clone();
        let current_duration_ms = services.current_duration_ms.clone();
        let current_dsd_rate = services.current_dsd_rate.clone();
        let is_playing = services.is_playing.clone();
        let remote_handle = services.remote_handle.clone();
        cx.set_global(services);
        crate::library_scan_indicator::setup(cx);
        {
            let (stored, initial_volume, show_device_picker) = {
                let store = cx.global::<crate::settings_store::SettingsStore>();
                (
                    store.playback().clone(),
                    store.volume(),
                    store.show_device_picker(),
                )
            };
            let services = cx.global::<Services>();
            services.output.set_volume(initial_volume);
            services.output.set_follow_default(!show_device_picker);
            // The one place the queue is rebuilt from disk: resolve the saved
            // locators against the catalog before taking the queue borrow.
            let repo = services.library.repo();
            let tracks = crate::services::restore_queue(&stored.queue, &*repo);
            let original_order = stored
                .original_queue
                .as_deref()
                .map(|refs| crate::services::restore_queue(refs, &*repo));
            {
                let mut queue = services.playback_queue.borrow_mut();
                queue.restore(crate::playback_queue::QueueRestore {
                    tracks,
                    original_order,
                    current_index: stored.current_index,
                    shuffle: stored.shuffle,
                    repeat: stored.repeat.into(),
                    source: stored.source.into(),
                    source_name: stored.source_name.map(SharedString::from),
                    custom: stored.custom,
                });
            }
        }

        crate::services::apply_remote_state(cx);

        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        cx.on_app_quit(|cx| {
            let history = crate::play_history::finalize_on_quit(cx);
            crate::discord_bridge::finalize_on_quit(cx);
            let state = cx.global::<Services>().snapshot_playback();
            let _ = cx
                .global_mut::<crate::settings_store::SettingsStore>()
                .save_playback_blocking(state);
            cx.global::<Services>().shutdown();
            diagnostics::flush();
            async move {
                if let Some(history) = history {
                    history.await;
                }
            }
        })
        .detach();

        cx.on_action(|_: &crate::app_menu::Quit, cx| {
            cx.quit();
        });

        cx.activate(true);
        crate::app_menu::set_menus(cx);

        crate::play_history::setup(cx);
        crate::sleep_timer::setup(cx);

        open_initial_window(cx, true);

        single_instance::install(cx, single_instance);

        crate::discord_bridge::setup(cx);

        cx.spawn(async move |cx| {
            run_engine_events_bus(
                cx,
                engine_manager,
                engine_event_bus,
                current_position_ms,
                current_duration_ms,
                current_dsd_rate,
                is_playing,
                remote_handle,
            )
            .await;
        })
        .detach();
    });
}
