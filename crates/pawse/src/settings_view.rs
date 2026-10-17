use std::path::PathBuf;

use gpui::{
    Anchor, AnyElement, App, ElementId, Entity, IntoElement, ParentElement, SharedString,
    Styled, Window, actions, div, px,
};
use gpui_component::{
    Disableable, Icon, IconName, Selectable, Sizable, WindowExt,
    button::{Button, ButtonGroup, ButtonVariants},
    dialog::{Cancel, Confirm, DialogFooter},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    notification::Notification,
    slider::{Slider, SliderState},
    switch::Switch,
    v_flex,
};

use ui_components::settings::{SettingField, SettingGroup, SettingItem, SettingPage, Settings};

use crate::localization::tr;
use crate::services::Services;
use crate::settings_store::{
    AlbumsArtistDisplay, AlbumsLayout, BlurBackground, FontScale, NowPlayingDetails, SettingsStore,
    StartupTab, WindowPreset, apply_font_scale, notify_save_error,
};
use crate::theme_colors::Colors;
use music_library::ArtistGrouping;

actions!(settings, [OpenOnlineSettings, OpenSleepTimerSettings]);

/// Width of the track-count column in the folder list, and the size of the
/// status icon that sits in front of each row.
const COUNT_COLUMN: f32 = 110.;
const ICON_SIZE: f32 = 16.;

fn reveal_in_file_manager(path: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[derive(Clone)]
pub struct SettingsSliders {
    pub lyrics: Entity<SliderState>,
    pub blur_intensity: Entity<SliderState>,
    pub blur_interface_opacity: Entity<SliderState>,
}

/// Build the list of `SettingPage`s for the Settings widget.
///
/// Built once and cached on `MainView`. `SettingPage` is `Clone` so the cache
/// is cloned into a fresh `Settings::new(...).pages(...)` shell on each render.
#[derive(Clone)]
pub struct LibraryPage {
    pub sources: Entity<crate::library_sources::LibrarySources>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_settings_pages(
    sliders: SettingsSliders,
    remote_port_input: Entity<InputState>,
    window_size_inputs: WindowSizeInputs,
    library_page: LibraryPage,
    sleep_timer_controls: Entity<crate::sleep_timer::controls::SleepTimerControls>,
    cx: &App,
) -> SettingsPages {
    let albums_layout = cx.global::<SettingsStore>().albums_layout();
    let blur_mode = cx.global::<SettingsStore>().blur_background();
    let window_preset = cx.global::<SettingsStore>().window_preset();
    let mut pages = vec![
        SettingPage::new(tr().settings_appearance.clone())
            .group(appearance_group(
                blur_mode,
                sliders.blur_intensity,
                sliders.blur_interface_opacity,
            ))
            .group(window_group(window_size_inputs, window_preset))
            .group(library_pages_group(albums_layout))
            .group(cover_view_group())
            .group(queue_group())
            .group(lyrics_group(sliders.lyrics))
            .group(now_playing_group()),
    ];
    let general = SettingPage::new(tr().settings_general.clone())
        .group(crate::sleep_timer::settings::settings_group(
            cx,
            sleep_timer_controls,
        ))
        .group(version_group());
    let general_ix = pages.len();
    pages.push(general);

    // Everything that reaches the network lives on this one page, and every
    // switch on it starts off. The rest of the app is fully usable without it.
    let mut online = SettingPage::new(tr().settings_online.clone()).group(online_lyrics_group());
    if discord::is_available() {
        online = online.group(discord_group());
    }
    online = online.group(remote_group(remote_port_input));
    let online_ix = pages.len();
    pages.push(online);

    let library = pages.len();
    pages.push(
        SettingPage::new(tr().settings_library.clone())
            .group(local_folders_group(library_page.sources.clone()))
            .group(backups_group())
            .group(portable_group()),
    );

    let animations = pages.len();
    pages.push(SettingPage::new(SharedString::from("Animations")).group(animations_group()));

    SettingsPages {
        pages,
        general: general_ix,
        library,
        online: online_ix,
        animations,
    }
}

#[derive(Clone)]
pub struct SettingsPages {
    pub pages: Vec<SettingPage>,
    pub general: usize,
    pub library: usize,
    pub online: usize,
    pub animations: usize,
}

/// Wrap pre-built pages into the `Settings` element for inline rendering.
pub fn settings_widget(
    pages: Vec<SettingPage>,
    page_ix: usize,
    anchor: Option<SharedString>,
    request: u64,
) -> Settings {
    Settings::new("pulse-settings")
        .pages(pages)
        .initial_page(page_ix)
        .initial_anchor(anchor)
        .page_request(request)
}

/// Open a native folder picker (async, on the main thread), then add the
/// chosen folder to `SettingsStore` and trigger a full clear+rescan over the
/// whole list. Shared between the Settings view and onboarding.
pub fn pick_and_add_folder(cx: &mut App) {
    cx.spawn(async move |cx| {
        if let Some(handle) = rfd::AsyncFileDialog::new().pick_folder().await {
            let path = handle.path().to_path_buf();
            cx.update(|cx| add_folder_and_rescan(path, cx));
        }
    })
    .detach();
}

/// Add a folder to settings (idempotent) and kick off a full rescan of all
/// configured folders. Surfaces save errors via `notify_save_error`.
pub fn add_folder_and_rescan(path: PathBuf, cx: &mut App) {
    let save_result = cx.global_mut::<SettingsStore>().add_music_folder(path);
    if let Err(e) = save_result {
        notify_save_error(cx, e);
    }
    let folders = cx.global::<SettingsStore>().music_folders().to_vec();
    cx.global::<Services>().library.clear_and_rescan(folders);
    crate::library_watcher::rebuild(cx);
}

pub fn force_rescan(cx: &mut App) {
    let folders = cx.global::<SettingsStore>().music_folders().to_vec();
    cx.global::<Services>()
        .library
        .request_rescan(folders, true, true);
}

pub fn confirm_remove_folder(path: PathBuf, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let path = path.clone();
        dialog
            .overlay_closable(false)
            .close_button(false)
            .title(tr().remove_folder_confirm_title.clone())
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("cancel")
                            .label(tr().cancel.clone())
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(Cancel), cx)),
                    )
                    .child(
                        Button::new("ok")
                            .label(tr().remove.clone())
                            .primary()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(Confirm { secondary: false }), cx)
                            }),
                    ),
            )
            .on_ok(move |_, _, cx| {
                remove_folder_and_rescan(path.clone(), cx);
                true
            })
    });
}

/// Remove a folder from settings and rescan whatever remains (a clear+rescan
/// is the simplest way to keep the DB consistent with the configured list).
pub fn remove_folder_and_rescan(path: PathBuf, cx: &mut App) {
    let save_result = cx.global_mut::<SettingsStore>().remove_music_folder(&path);
    if let Err(e) = save_result {
        notify_save_error(cx, e);
    }
    let folders = cx.global::<SettingsStore>().music_folders().to_vec();
    cx.global::<Services>().library.clear_and_rescan(folders);
    crate::library_watcher::rebuild(cx);
}

fn appearance_group(
    blur_mode: BlurBackground,
    blur_intensity_slider: Entity<SliderState>,
    blur_interface_opacity_slider: Entity<SliderState>,
) -> SettingGroup {
    let mut group = SettingGroup::new().title(tr().settings_interface.clone());

    group = group.item(
        SettingItem::new(
            tr().dynamic_theme.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let enabled = cx.global::<SettingsStore>().dynamic_theme();
                h_flex().items_center().justify_end().child(
                    Switch::new("dynamic-theme-toggle")
                        .checked(enabled)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) =
                                cx.global_mut::<SettingsStore>().set_dynamic_theme(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                        }),
                )
            }),
        )
        .description(tr().dynamic_theme_desc.clone()),
    );

    group = group.item(
        SettingItem::new(
            tr().font_size.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let current = cx.global::<SettingsStore>().font_scale();
                h_flex().items_center().justify_end().child(
                    ButtonGroup::new("font-size-group")
                        .small()
                        .child(
                            Button::new("font-small")
                                .label(tr().font_size_small.clone())
                                .selected(current == FontScale::Small),
                        )
                        .child(
                            Button::new("font-medium")
                                .label(tr().font_size_medium.clone())
                                .selected(current == FontScale::Medium),
                        )
                        .child(
                            Button::new("font-large")
                                .label(tr().font_size_large.clone())
                                .selected(current == FontScale::Large),
                        )
                        .on_click(|clicks: &Vec<usize>, _, cx| {
                            let Some(&ix) = clicks.first() else {
                                return;
                            };
                            let scale = match ix {
                                0 => FontScale::Small,
                                2 => FontScale::Large,
                                _ => FontScale::Medium,
                            };
                            if let Err(e) = cx.global_mut::<SettingsStore>().set_font_scale(scale) {
                                notify_save_error(cx, e);
                            }
                            apply_font_scale(scale, cx);
                            cx.refresh_windows();
                        }),
                )
            }),
        )
        .description(tr().font_size_desc.clone()),
    );

    group = group.item(
        SettingItem::new(
            tr().blur_background.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let current = cx.global::<SettingsStore>().blur_background();
                h_flex().items_center().justify_end().child(
                    ButtonGroup::new("blur-background-group")
                        .small()
                        .child(
                            Button::new("blur-off")
                                .label(tr().blur_background_off.clone())
                                .selected(current == BlurBackground::Off),
                        )
                        .child(
                            Button::new("blur-cover")
                                .label(tr().blur_background_cover.clone())
                                .selected(current == BlurBackground::CoverView),
                        )
                        .child(
                            Button::new("blur-all")
                                .label(tr().blur_background_all.clone())
                                .selected(current == BlurBackground::AllViews),
                        )
                        .on_click(|clicks: &Vec<usize>, _, cx| {
                            let Some(&ix) = clicks.first() else {
                                return;
                            };
                            let mode = match ix {
                                0 => BlurBackground::Off,
                                2 => BlurBackground::AllViews,
                                _ => BlurBackground::CoverView,
                            };
                            if let Err(e) =
                                cx.global_mut::<SettingsStore>().set_blur_background(mode)
                            {
                                notify_save_error(cx, e);
                            }
                        }),
                )
            }),
        )
        .description(tr().blur_background_desc.clone()),
    );

    if blur_mode != BlurBackground::Off {
        group = group.item(
            SettingItem::new(
                tr().blur_intensity.clone(),
                SettingField::render(move |_window, _cx: &mut App| {
                    h_flex()
                        .items_center()
                        .justify_end()
                        .child(div().w(px(160.)).child(Slider::new(&blur_intensity_slider)))
                }),
            )
            .description(tr().blur_intensity_desc.clone()),
        );

        group = group.item(
            SettingItem::new(
                tr().blur_interface_opacity.clone(),
                SettingField::render(move |_window, _cx: &mut App| {
                    h_flex().items_center().justify_end().child(
                        div()
                            .w(px(160.))
                            .child(Slider::new(&blur_interface_opacity_slider)),
                    )
                }),
            )
            .description(tr().blur_interface_opacity_desc.clone()),
        );
    }

    // The untouched-signal-path toggle: exclusive on macOS/Windows, native
    // sample rate on Linux. Hidden when the platform can't offer it at all
    // (Linux without a running PipeWire server).
    if audio_output::native_mode_available() {
        let (label, description) = if cfg!(target_os = "linux") {
            (
                tr().native_rate_button.clone(),
                tr().native_rate_button_desc.clone(),
            )
        } else {
            (
                tr().exclusive_mode_button.clone(),
                tr().exclusive_mode_button_desc.clone(),
            )
        };
        group = group.item(
            SettingItem::new(
                label,
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_hog_button();
                    h_flex().items_center().justify_end().child(
                        Switch::new("exclusive-mode-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_hog_button(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(description),
        );
    }

    group
        .item(
            SettingItem::new(
                tr().device_picker_button.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_device_picker();
                    h_flex().items_center().justify_end().child(
                        Switch::new("device-picker-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_device_picker(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                                let output = &cx.global::<Services>().output;
                                output.set_follow_default(!*new_val);
                                if !*new_val {
                                    output.follow_system_default();
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().device_picker_button_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().repeat_shuffle.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_repeat_shuffle();
                    h_flex().items_center().justify_end().child(
                        Switch::new("repeat-shuffle-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_repeat_shuffle(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().repeat_shuffle_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().time_labels.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_time_labels();
                    h_flex().items_center().justify_end().child(
                        Switch::new("time-labels-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_time_labels(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().time_labels_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().tab_genres.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().genres_enabled();
                    h_flex().items_center().justify_end().child(
                        Switch::new("genres-enabled-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_genres_enabled(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().genres_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().tab_folders.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().folders_enabled();
                    h_flex().items_center().justify_end().child(
                        Switch::new("folders-enabled-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_folders_enabled(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().folders_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().tab_playlists.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().playlists_enabled();
                    h_flex().items_center().justify_end().child(
                        Switch::new("playlists-enabled-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_playlists_enabled(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().playlists_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().tab_rewind.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().rewind_enabled();
                    h_flex().items_center().justify_end().child(
                        Switch::new("rewind-enabled-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_rewind_enabled(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().rewind_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().tag_editor.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().tag_editor_enabled();
                    h_flex().items_center().justify_end().child(
                        Switch::new("tag-editor-toggle").checked(enabled).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_tag_editor_enabled(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().tag_editor_desc.clone()),
        )
}

/// The online-lyrics switches. Both go dark when the app is offline.
fn online_lyrics_group() -> SettingGroup {
    let mut group = SettingGroup::new().title(tr().settings_lyrics_online.clone());

    group = group.item(
        SettingItem::new(
            tr().lyrics_from_internet.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let enabled = cx.global::<SettingsStore>().lyrics_from_internet();
                h_flex().items_center().justify_end().child(
                    Switch::new("lyrics-from-internet-toggle")
                        .checked(enabled)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) = cx
                                .global_mut::<SettingsStore>()
                                .set_lyrics_from_internet(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                        }),
                )
            }),
        )
        .description(tr().lyrics_from_internet_desc.clone()),
    );

    group = group.item(
        SettingItem::new(
            tr().lyrics_prefer_lrclib.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let settings = cx.global::<SettingsStore>();
                let enabled = settings.lyrics_prefer_lrclib_setting();
                let online = settings.lyrics_from_internet();
                h_flex().items_center().justify_end().child(
                    Switch::new("lyrics-prefer-lrclib-toggle")
                        .checked(enabled && online)
                        .disabled(!online)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) = cx
                                .global_mut::<SettingsStore>()
                                .set_lyrics_prefer_lrclib(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                        }),
                )
            }),
        )
        .description(tr().lyrics_prefer_lrclib_desc.clone()),
    );

    group
}

fn remote_group(remote_port_input: Entity<InputState>) -> SettingGroup {
    let mut group = SettingGroup::new();

    group = group.item(
        SettingItem::new(
            tr().remote_control.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let enabled = cx.global::<SettingsStore>().remote_enabled();
                h_flex().items_center().justify_end().child(
                    Switch::new("remote-enabled-toggle")
                        .checked(enabled)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) = cx
                                .global_mut::<SettingsStore>()
                                .set_remote_enabled(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                            crate::services::apply_remote_state(cx);
                        }),
                )
            }),
        )
        .description(tr().remote_control_desc.clone()),
    );

    group = group.item(
        SettingItem::new(
            tr().remote_port.clone(),
            SettingField::render(move |_window, cx: &mut App| {
                let enabled = cx.global::<SettingsStore>().remote_enabled();
                h_flex()
                    .items_center()
                    .gap_2()
                    .justify_end()
                    .child(
                        div()
                            .w(px(120.))
                            .child(Input::new(&remote_port_input).small()),
                    )
                    .child(
                        Button::new("open-web-player")
                            .small()
                            .disabled(!enabled)
                            .icon(Icon::default().path("icons/external-link.svg"))
                            .tooltip(tr().open_in_browser.clone())
                            .on_click(|_, _, cx| {
                                let port = cx.global::<SettingsStore>().remote_port();
                                cx.open_url(&crate::net::remote_url(port));
                            }),
                    )
            }),
        )
        .description(tr().remote_port_desc.clone()),
    );

    // The address itself, shown rather than only opened. The whole point of
    // using the LAN address is that it can be read off this screen and typed
    // into a phone; `localhost` would be useless for exactly that.
    group = group.item(
        SettingItem::new(
            tr().remote_address.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let port = cx.global::<SettingsStore>().remote_port();
                h_flex().items_center().justify_end().child(
                    div()
                        .text_sm()
                        .text_color(Colors::muted_foreground(cx))
                        .child(crate::net::remote_url(port)),
                )
            }),
        )
        .description(tr().remote_address_desc.clone()),
    );

    group
}

fn version_group() -> SettingGroup {
    SettingGroup::new()
        .item(SettingItem::new(
            tr().version.clone(),
            SettingField::render(|_window, cx: &mut App| {
                h_flex().items_center().justify_end().child(
                    div()
                        .text_sm()
                        .text_color(Colors::muted_foreground(cx))
                        .child(env!("CARGO_PKG_VERSION")),
                )
            }),
        ))
        // A signature, not UI copy: a literal rather than a translation key, and
        // deliberately not interactive.
        .item(SettingItem::new(
            SharedString::from("Made by"),
            SettingField::render(|_window, cx: &mut App| {
                h_flex().items_center().justify_end().child(
                    div()
                        .text_sm()
                        .text_color(Colors::muted_foreground(cx))
                        .child("blonded"),
                )
            }),
        ))
}

/// Pulse 5.0 is the animation release. This page exists now so the intention has
/// a home, a name and a written anchor; it has no controls on purpose and
/// nothing in the app depends on it.
fn animations_group() -> SettingGroup {
    let mut group = SettingGroup::new()
        .title(SharedString::from("Motion"))
        .item(
            SettingItem::new(
                SharedString::from("Play animations"),
                SettingField::render(|_window, cx: &mut App| {
                    h_flex().items_center().justify_end().child(
                        Switch::new("animations-master")
                            .checked(cx.global::<SettingsStore>().animations().enabled)
                            .on_click(|new_val, _, cx| {
                                let mut prefs = cx.global::<SettingsStore>().animations();
                                prefs.enabled = *new_val;
                                if let Err(e) =
                                    cx.global_mut::<SettingsStore>().set_animations(prefs)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(SharedString::from("Turns every animation off at once.")),
        );

    // Driven by the same list the call sites use, so a new animation cannot be
    // added to the code and be missing from this page.
    for (ix, kind) in crate::animations::AnimationKind::ALL.into_iter().enumerate() {
        group = group.item(
            SettingItem::new(
                SharedString::from(kind.label()),
                SettingField::render(move |_window, cx: &mut App| {
                    let on = kind_setting(kind, cx.global::<SettingsStore>().animations());
                    h_flex().items_center().justify_end().child(
                        Switch::new(ElementId::NamedInteger("animations-kind".into(), ix as u64))
                            .checked(on)
                            .on_click(move |new_val, _, cx| {
                                let mut prefs = cx.global::<SettingsStore>().animations();
                                set_kind(&mut prefs, kind, *new_val);
                                if let Err(e) =
                                    cx.global_mut::<SettingsStore>().set_animations(prefs)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(SharedString::from(kind.description())),
        );
    }

    group
}

fn kind_setting(
    kind: crate::animations::AnimationKind,
    prefs: crate::settings_store::AnimationSettings,
) -> bool {
    use crate::animations::AnimationKind as K;
    match kind {
        K::PanelSlide => prefs.panel_slide,
        K::ViewFade => prefs.view_fade,
        K::TrackChange => prefs.track_change,
        K::CoverMode => prefs.cover_mode,
    }
}

fn set_kind(
    prefs: &mut crate::settings_store::AnimationSettings,
    kind: crate::animations::AnimationKind,
    on: bool,
) {
    use crate::animations::AnimationKind as K;
    match kind {
        K::PanelSlide => prefs.panel_slide = on,
        K::ViewFade => prefs.view_fade = on,
        K::TrackChange => prefs.track_change = on,
        K::CoverMode => prefs.cover_mode = on,
    }
}

fn discord_group() -> SettingGroup {
    SettingGroup::new()
        .title(SharedString::from("Discord"))
        .item(
            SettingItem::new(
                tr().discord_share.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().discord_enabled();
                    h_flex().items_center().justify_end().child(
                        Switch::new("discord-enabled-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_discord_enabled(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                                crate::discord_bridge::set_enabled(cx, *new_val);
                            }),
                    )
                }),
            )
            .description(tr().discord_share_desc.clone()),
        )
}

fn now_playing_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().settings_now_playing.clone())
        .item(
            SettingItem::new(
                tr().now_playing_details.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let current = cx.global::<SettingsStore>().now_playing_details();
                    h_flex().items_center().justify_end().child(
                        ButtonGroup::new("now-playing-details-group")
                            .small()
                            .child(
                                Button::new("now-playing-details-specs")
                                    .label(tr().now_playing_details_specs.clone())
                                    .selected(current == NowPlayingDetails::Specs),
                            )
                            .child(
                                Button::new("now-playing-details-year")
                                    .label(tr().album_year.clone())
                                    .selected(current == NowPlayingDetails::Year),
                            )
                            .child(
                                Button::new("now-playing-details-album")
                                    .label(tr().now_playing_details_album.clone())
                                    .selected(current == NowPlayingDetails::Album),
                            )
                            .child(
                                Button::new("now-playing-details-hidden")
                                    .label(tr().albums_artist_hidden.clone())
                                    .selected(current == NowPlayingDetails::Hidden),
                            )
                            .on_click(|clicks: &Vec<usize>, _, cx| {
                                let Some(&ix) = clicks.first() else {
                                    return;
                                };
                                let details = match ix {
                                    1 => NowPlayingDetails::Year,
                                    2 => NowPlayingDetails::Album,
                                    3 => NowPlayingDetails::Hidden,
                                    _ => NowPlayingDetails::Specs,
                                };
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_now_playing_details(details)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().now_playing_details_desc.clone()),
        )
}

fn artists_grouping_item() -> SettingItem {
    SettingItem::new(
        tr().artists_group_by_tag.clone(),
        SettingField::render(|_window, cx: &mut App| {
            let current = cx.global::<SettingsStore>().artists_grouping();
            h_flex().items_center().justify_end().child(
                ButtonGroup::new("artists-grouping-group")
                    .small()
                    .child(
                        Button::new("artists-grouping-track-artist")
                            .label("artist")
                            .selected(current == ArtistGrouping::TrackArtist),
                    )
                    .child(
                        Button::new("artists-grouping-album-artist")
                            .label("album artist")
                            .selected(current == ArtistGrouping::AlbumArtist),
                    )
                    .on_click(|clicks: &Vec<usize>, _, cx| {
                        let Some(&ix) = clicks.first() else {
                            return;
                        };
                        let grouping = match ix {
                            1 => ArtistGrouping::AlbumArtist,
                            _ => ArtistGrouping::TrackArtist,
                        };
                        if let Err(e) = cx
                            .global_mut::<SettingsStore>()
                            .set_artists_grouping(grouping)
                        {
                            notify_save_error(cx, e);
                        }
                    }),
            )
        }),
    )
}

fn library_pages_group(layout: AlbumsLayout) -> SettingGroup {
    let list_mode = layout == AlbumsLayout::List;
    let mut group = SettingGroup::new()
        .title(tr().settings_library_pages.clone())
        .item(
            SettingItem::new(
                tr().albums_layout.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let current = cx.global::<SettingsStore>().albums_layout();
                    h_flex().items_center().justify_end().child(
                        ButtonGroup::new("albums-layout-group")
                            .small()
                            .child(
                                Button::new("albums-layout-list")
                                    .label(tr().albums_layout_list.clone())
                                    .selected(current == AlbumsLayout::List),
                            )
                            .child(
                                Button::new("albums-layout-grid")
                                    .label(tr().albums_layout_grid.clone())
                                    .selected(current == AlbumsLayout::Grid),
                            )
                            .on_click(|clicks: &Vec<usize>, _, cx| {
                                let Some(&ix) = clicks.first() else {
                                    return;
                                };
                                let layout = match ix {
                                    1 => AlbumsLayout::Grid,
                                    _ => AlbumsLayout::List,
                                };
                                if let Err(e) =
                                    cx.global_mut::<SettingsStore>().set_albums_layout(layout)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().albums_layout_desc.clone()),
        );

    if list_mode {
        group = group.item(
            SettingItem::new(
                tr().artist_name.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let current = cx.global::<SettingsStore>().albums_artist_display();
                    h_flex().items_center().justify_end().child(
                        ButtonGroup::new("albums-artist-group")
                            .small()
                            .child(
                                Button::new("albums-artist-inline")
                                    .label(tr().albums_artist_inline.clone())
                                    .selected(current == AlbumsArtistDisplay::Inline),
                            )
                            .child(
                                Button::new("albums-artist-column")
                                    .label(tr().albums_artist_column.clone())
                                    .selected(current == AlbumsArtistDisplay::Column),
                            )
                            .child(
                                Button::new("albums-artist-hidden")
                                    .label(tr().albums_artist_hidden.clone())
                                    .selected(current == AlbumsArtistDisplay::Hidden),
                            )
                            .on_click(|clicks: &Vec<usize>, _, cx| {
                                let Some(&ix) = clicks.first() else {
                                    return;
                                };
                                let display = match ix {
                                    1 => AlbumsArtistDisplay::Column,
                                    2 => AlbumsArtistDisplay::Hidden,
                                    _ => AlbumsArtistDisplay::Inline,
                                };
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_albums_artist_display(display)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().albums_artist_desc.clone()),
        );
    }

    group = group.item(
        SettingItem::new(
            if list_mode {
                tr().year_column.clone()
            } else {
                tr().album_year.clone()
            },
            SettingField::render(|_window, cx: &mut App| {
                let show = cx.global::<SettingsStore>().albums_show_year();
                h_flex().items_center().justify_end().child(
                    Switch::new("albums-year-column-toggle")
                        .checked(show)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) = cx
                                .global_mut::<SettingsStore>()
                                .set_albums_show_year(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                        }),
                )
            }),
        )
        .description(if list_mode {
            tr().year_column_desc.clone()
        } else {
            tr().album_year_desc.clone()
        }),
    );

    if list_mode {
        group = group.item(
            SettingItem::new(
                tr().genre_column.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().albums_show_genre();
                    h_flex().items_center().justify_end().child(
                        Switch::new("albums-genre-column-toggle")
                            .checked(show)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_albums_show_genre(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().genre_column_desc.clone()),
        );
    }

    // The artists row belongs here too: both are "how the library pages show
    // things", and a card holding a single row reads as clutter.
    group.item(artists_grouping_item())
}

fn cover_view_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().cover_mode.clone())
        .item(
            SettingItem::new(
                tr().artist_name.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().cover_show_artist();
                    h_flex().items_center().justify_end().child(
                        Switch::new("cover-artist-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_cover_show_artist(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().cover_artist_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().cover_progress.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().cover_show_progress();
                    h_flex().items_center().justify_end().child(
                        Switch::new("cover-progress-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_cover_show_progress(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().cover_progress_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().cover_controls.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().cover_show_controls();
                    h_flex().items_center().justify_end().child(
                        Switch::new("cover-controls-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_cover_show_controls(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().cover_controls_desc.clone()),
        )
}

fn queue_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().queue.clone())
        .item(
            SettingItem::new(
                tr().track_duration.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_track_duration();
                    h_flex().items_center().justify_end().child(
                        Switch::new("track-duration-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_track_duration(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().track_duration_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().action_buttons.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_queue_actions();
                    h_flex().items_center().justify_end().child(
                        Switch::new("queue-actions-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_queue_actions(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().action_buttons_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().artist_name.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let show = cx.global::<SettingsStore>().show_queue_artist();
                    h_flex().items_center().justify_end().child(
                        Switch::new("queue-artist-toggle").checked(show).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_show_queue_artist(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().artist_name_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().queue_deduplication.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().queue_deduplication();
                    h_flex().items_center().justify_end().child(
                        Switch::new("queue-dedup-toggle").checked(enabled).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_queue_deduplication(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(tr().queue_deduplication_desc.clone()),
        )
}

fn lyrics_group(slider: Entity<SliderState>) -> SettingGroup {
    SettingGroup::new()
        .title(tr().lyrics.clone())
        .item(
            SettingItem::new(
                tr().lyrics_text_size.clone(),
                SettingField::render(move |_window, cx: &mut App| {
                    let size = slider.read(cx).value().start();
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .w(px(38.))
                                .text_sm()
                                .text_color(Colors::muted_foreground(cx))
                                .child(format!("{} px", size as i32)),
                        )
                        .child(div().w(px(160.)).child(Slider::new(&slider)))
                }),
            )
            .description(tr().lyrics_text_size_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().lyrics_karaoke.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().lyrics_karaoke_fill();
                    h_flex().items_center().justify_end().child(
                        Switch::new("lyrics-karaoke-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_lyrics_karaoke_fill(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().lyrics_karaoke_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().lyrics_dim_inactive.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let enabled = cx.global::<SettingsStore>().lyrics_dim_inactive();
                    h_flex().items_center().justify_end().child(
                        Switch::new("lyrics-dim-inactive-toggle")
                            .checked(enabled)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_lyrics_dim_inactive(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            }),
                    )
                }),
            )
            .description(tr().lyrics_dim_inactive_desc.clone()),
        )
}

/// Export a copy of the program that carries its own data.
fn portable_group() -> SettingGroup {
    SettingGroup::new()
        .title(SharedString::from("Portable copy"))
        .item(
            SettingItem::new(
                SharedString::from("Include settings"),
                SettingField::render(|_window, cx: &mut App| {
                    let on = cx.global::<SettingsStore>().export_include_settings();
                    h_flex().items_center().justify_end().child(
                        Switch::new("export-include-settings").checked(on).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_export_include_settings(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(SharedString::from(
                "Theme, layout, queue and preferences.",
            )),
        )
        .item(
            SettingItem::new(
                SharedString::from("Include library"),
                SettingField::render(|_window, cx: &mut App| {
                    let on = cx.global::<SettingsStore>().export_include_library();
                    h_flex().items_center().justify_end().child(
                        Switch::new("export-include-library").checked(on).on_click(
                            |new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_export_include_library(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                            },
                        ),
                    )
                }),
            )
            .description(SharedString::from(
                "Tracks, playlists and play history. Usually large; leave it off for a bare copy.",
            )),
        )
        .item(
            SettingItem::new(
                SharedString::from("Export Pulse…"),
                SettingField::render(|_window, _cx: &mut App| {
                    h_flex().items_center().justify_end().child(
                        Button::new("export-portable")
                            .small()
                            .label(SharedString::from("Export…"))
                            .on_click(|_, _, cx| start_export(cx)),
                    )
                }),
            )
            .description(SharedString::from(
                "Copies the program into a folder you can move to another machine. Run Pulse.sh inside it and the copy uses its own data.",
            )),
        )
}

fn start_export(cx: &mut App) {
    let options = crate::portable::ExportOptions {
        settings: cx.global::<SettingsStore>().export_include_settings(),
        library: cx.global::<SettingsStore>().export_include_library(),
    };
    cx.spawn(async move |cx| {
        let Some(handle) = rfd::AsyncFileDialog::new()
            .set_title("Choose where to export Pulse")
            .pick_folder()
            .await
        else {
            return;
        };
        let destination = handle.path().to_path_buf();
        let result = crate::portable::export(&destination, options);
        cx.update(|cx| {
            let note = match result {
                Ok(root) => Notification::success(format!("Exported to {}", root.display())),
                Err(err) => Notification::warning(format!("Export failed: {err}")),
            };
            let note = note.title(SharedString::from("Portable copy"));
            // There is no window in this async task, so go through the active
            // one — the same route `notify_save_error` takes.
            if let Some(handle) = cx.active_window() {
                let _ = handle.update(cx, |_, window, cx| {
                    window.push_notification(note, cx);
                });
            }
        });
    })
    .detach();
}

/// Backups and the health check — the two things that make it safe to keep the
/// whole library in a single file.
fn backups_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().backups.clone())
        .item(
            SettingItem::new(
                tr().restore_backup.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let backups = cx.global::<Services>().library.repo().backups();
                    let summary = match backups.first() {
                        Some(newest) => {
                            format!("{} saved · newest {}", backups.len(), newest.name)
                        }
                        None => {
                            "none yet — one is written on the first launch of each day".to_string()
                        }
                    };
                    h_flex()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(Colors::muted_foreground(cx))
                                .child(summary),
                        )
                        .child(
                            Button::new("restore-backup")
                                .small()
                                .label(tr().restore_backup.clone())
                                .disabled(backups.is_empty())
                                .on_click(|_, window, cx| open_restore_dialog(window, cx)),
                        )
                }),
            )
            .description(tr().restore_backup_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().library_check.clone(),
                SettingField::render(|_window, _cx: &mut App| {
                    h_flex().items_center().justify_end().child(
                        Button::new("library-check")
                            .small()
                            .label(tr().check_now.clone())
                            .on_click(|_, window, cx| run_library_check(window, cx)),
                    )
                }),
            )
            .description(tr().library_check_desc.clone()),
        )
}

/// List the backups and let one be picked. Restoring happens on the next launch,
/// so this only records the request — see `LibraryRepository::request_restore`.
fn open_restore_dialog(window: &mut Window, cx: &mut App) {
    let backups = cx.global::<Services>().library.repo().backups();
    if backups.is_empty() {
        return;
    }
    window.open_dialog(cx, move |dialog, _, _| {
        let mut list = v_flex().gap_1().w_full();
        for entry in &backups {
            let name = entry.name.clone();
            let path = entry.path.clone();
            list = list.child(
                Button::new(format!("restore-{name}"))
                    .small()
                    .label(name)
                    .on_click({
                        let path = path.clone();
                        move |_, window, cx| {
                            schedule_restore(path.clone(), window, cx);
                            window.close_dialog(cx);
                        }
                    }),
            );
        }
        dialog
            .title(tr().restore_backup.clone())
            .w(px(460.))
            .child(div().child(tr().restore_backup_desc.clone()))
            .child(list)
    });
}

fn schedule_restore(path: PathBuf, window: &mut Window, cx: &mut App) {
    let result = cx
        .global::<Services>()
        .library
        .repo()
        .request_restore(&path);
    let notification = match result {
        Ok(()) => Notification::success(tr().restore_scheduled.clone()),
        Err(err) => Notification::error(format!("{err}")),
    };
    window.push_notification(notification.title(tr().restore_backup.clone()), cx);
}

fn run_library_check(window: &mut Window, cx: &mut App) {
    let report = cx.global::<Services>().library.repo().integrity_report();
    let notification = match report {
        Ok(report) if report.is_clean() => {
            Notification::success(tr().library_check_clean.clone())
        }
        Ok(report) => Notification::warning(format!(
            "Integrity: {}\nForeign key violations: {}\nBroken playlist entries: {}\nOrphaned catalog rows: {}",
            report.integrity,
            report.foreign_key_violations,
            report.dangling_playlist_entries,
            report.orphaned_catalog_rows,
        )),
        Err(err) => Notification::error(format!("{err}")),
    };
    window.push_notification(notification.title(tr().library_check.clone()), cx);
}

fn local_folders_group(sources: Entity<crate::library_sources::LibrarySources>) -> SettingGroup {
    SettingGroup::new()
        .title(tr().local_folders.clone())
        .item(SettingItem::unlabeled(SettingField::render(
            move |_window, cx: &mut App| {
                let state = sources.read(cx);
                let is_scanning = cx.global::<Services>().library.is_scanning();
                let muted_fg = Colors::muted_foreground(cx);

                let mut list = v_flex().gap_2().w_full();

                for folder in state.local() {
                    let path = &folder.row.path;
                    let path_for_finder = path.clone();
                    let path_for_remove = path.clone();
                    let finder_id = format!("show-{}", path.display());
                    let remove_id = format!("remove-{}", path.display());
                    let status_color = match folder.row.status {
                        crate::library_sources::SourceStatus::Online => Colors::primary(cx),
                        crate::library_sources::SourceStatus::Offline => Colors::danger(cx),
                        crate::library_sources::SourceStatus::Scanning => muted_fg,
                    };

                    list = list.child(
                        v_flex()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .rounded(px(6.))
                            .bg(Colors::muted(cx))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .with_size(px(ICON_SIZE))
                                            .flex_shrink_0()
                                            .text_color(muted_fg),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .text_sm()
                                            .text_color(Colors::foreground(cx))
                                            .child(folder.row.location.clone()),
                                    )
                                    .child(
                                        Button::new(SharedString::from(finder_id))
                                            .small()
                                            .label(tr().reveal_folder.clone())
                                            .on_click(move |_, _, _| {
                                                reveal_in_file_manager(&path_for_finder);
                                            }),
                                    )
                                    .child(
                                        Button::new(SharedString::from(remove_id))
                                            .small()
                                            .label(tr().remove.clone())
                                            .on_click(
                                                move |_, window: &mut Window, app_cx: &mut App| {
                                                    confirm_remove_folder(
                                                        path_for_remove.clone(),
                                                        window,
                                                        app_cx,
                                                    );
                                                },
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .pl(px(ICON_SIZE + 8.))
                                    .items_center()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .w(px(COUNT_COLUMN))
                                            .child(folder.count_label.clone()),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        h_flex()
                                            .flex_shrink_0()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                div().size(px(8.)).rounded_full().bg(status_color),
                                            )
                                            .child(
                                                div()
                                                    .text_color(status_color)
                                                    .child(folder.status_label.clone()),
                                            ),
                                    ),
                            ),
                    );
                }

                let list = (!state.local().is_empty()).then_some(list);
                v_flex().gap_3().w_full().children(list).child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("add-folder")
                                .small()
                                .label(tr().add_folder.clone())
                                .on_click(|_, _, cx| pick_and_add_folder(cx)),
                        )
                        .child(
                            Button::new("rescan-library")
                                .small()
                                .disabled(is_scanning)
                                .label(tr().rescan_library.clone())
                                .on_click(|_, _, cx| force_rescan(cx)),
                        ),
                )
            },
        )))
}

/// The custom-size inputs live in `MainView` next to the remote-port input:
/// an `InputState` needs a window to exist, and the settings pages are built
/// once and cached.
#[derive(Clone)]
pub struct WindowSizeInputs {
    pub width: Entity<InputState>,
    pub height: Entity<InputState>,
}

fn window_group(inputs: WindowSizeInputs, preset: WindowPreset) -> SettingGroup {
    let width_input = inputs.width.clone();
    let height_input = inputs.height.clone();
    let group = SettingGroup::new()
        .title(tr().window_group.clone())
        .item(startup_tab_field())
        .item(window_size_field());
    // The two boxes describe the Custom preset, so they would only be noise
    // while a fraction of the screen is in charge. The group is rebuilt
    // whenever the store changes, which is what makes this appear and vanish.
    if preset != WindowPreset::Custom {
        return group;
    }
    group
        .item(SettingItem::new(
            tr().window_width.clone(),
            SettingField::render(move |_window, _cx: &mut App| {
                h_flex()
                    .items_center()
                    .justify_end()
                    .child(div().w(px(120.)).child(Input::new(&width_input).small()))
            }),
        ))
        .item(SettingItem::new(
            tr().window_height.clone(),
            SettingField::render(move |_window, _cx: &mut App| {
                h_flex()
                    .items_center()
                    .justify_end()
                    .child(div().w(px(120.)).child(Input::new(&height_input).small()))
            }),
        ))
}

fn startup_tab_field() -> SettingItem {
    SettingItem::new(
        tr().startup_tab.clone(),
        SettingField::render(|_window, cx: &mut App| startup_tab_dropdown(cx)),
    )
    .description(tr().startup_tab_desc.clone())
}

/// The tab names live next to the tabs themselves; only the enabled ones are
/// offered, since a disabled tab is not there to land on.
fn startup_tab_menu(menu: PopupMenu, cx: &App) -> PopupMenu {
    let store = cx.global::<SettingsStore>();
    let current = store.startup_tab();
    let offered = [
        (StartupTab::Albums, tr().tab_albums.clone()),
        (StartupTab::Artists, tr().tab_artists.clone()),
        (StartupTab::Genres, tr().tab_genres.clone()),
        (StartupTab::AllTracks, tr().all_tracks.clone()),
        (StartupTab::Folders, tr().tab_folders.clone()),
        (StartupTab::Playlists, tr().tab_playlists.clone()),
    ];
    offered
        .into_iter()
        .filter(|(tab, _)| {
            crate::library_views::library_view::LibraryRootTab::from_startup(*tab)
                .is_available(store)
        })
        .fold(menu, |menu, (tab, label)| {
            menu.item(PopupMenuItem::new(label).checked(tab == current).on_click(
                move |_, _, cx| {
                    if let Err(e) = cx.global_mut::<SettingsStore>().set_startup_tab(tab) {
                        notify_save_error(cx, e);
                    }
                },
            ))
        })
}

fn startup_tab_dropdown(cx: &App) -> AnyElement {
    let label = startup_tab_label(cx.global::<SettingsStore>().startup_tab());
    Button::new("startup-tab-dropdown")
        .small()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Anchor::TopRight, |menu, _, cx| startup_tab_menu(menu, cx))
        .into_any_element()
}

fn startup_tab_label(tab: StartupTab) -> SharedString {
    match tab {
        StartupTab::Albums => tr().tab_albums.clone(),
        StartupTab::Artists => tr().tab_artists.clone(),
        StartupTab::Genres => tr().tab_genres.clone(),
        StartupTab::AllTracks => tr().all_tracks.clone(),
        StartupTab::Folders => tr().tab_folders.clone(),
        StartupTab::Playlists => tr().tab_playlists.clone(),
    }
}

fn window_size_field() -> SettingItem {
    SettingItem::new(
        tr().window_size.clone(),
        SettingField::render(|_window, cx: &mut App| window_size_dropdown(cx)),
    )
    .description(tr().window_size_desc.clone())
}

fn window_size_menu(menu: PopupMenu, cx: &App) -> PopupMenu {
    let current = cx.global::<SettingsStore>().window_preset();
    [
        (WindowPreset::Landscape, tr().window_size_landscape.clone()),
        (WindowPreset::Portrait, tr().window_size_portrait.clone()),
        (WindowPreset::Custom, tr().window_size_custom.clone()),
    ]
    .into_iter()
    .fold(menu, |menu, (preset, label)| {
        menu.item(
            PopupMenuItem::new(label)
                .checked(preset == current)
                .on_click(move |_, _, cx| {
                    if let Err(e) = cx.global_mut::<SettingsStore>().set_window_preset(preset) {
                        notify_save_error(cx, e);
                    }
                }),
        )
    })
}

fn window_size_dropdown(cx: &App) -> AnyElement {
    let preset = cx.global::<SettingsStore>().window_preset();
    let label = match preset {
        WindowPreset::Landscape => tr().window_size_landscape.clone(),
        WindowPreset::Portrait => tr().window_size_portrait.clone(),
        WindowPreset::Custom => {
            let (w, h) = cx.global::<SettingsStore>().custom_window_size();
            format!("{w:.0} \u{d7} {h:.0}").into()
        }
    };
    Button::new("window-size-dropdown")
        .small()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Anchor::TopRight, |menu, _, cx| window_size_menu(menu, cx))
        .into_any_element()
}
