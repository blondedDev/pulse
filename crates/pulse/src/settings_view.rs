use std::path::PathBuf;

use gpui::{
    Anchor, AnyElement, App, Axis, ElementId, Entity, IntoElement, ParentElement, SharedString,
    Styled, Window, actions, div, img, px,
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

use gpui::prelude::FluentBuilder;
use ui_components::settings::{SettingField, SettingGroup, SettingItem, SettingPage, Settings};

use crate::localization::tr;
use crate::services::Services;
use crate::settings_store::{
    AlbumsArtistDisplay, AlbumsLayout, BlurBackground, FontScale, NowPlayingDetails, SettingsStore,
    StartupTab, WindowPreset, apply_font_scale, notify_save_error,
};
use crate::theme_colors::Colors;
use music_library::ArtistGrouping;

actions!(settings, [OpenOnlineSettings]);

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
        .group(playback_group())
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
    pages.push(SettingPage::new(tr().settings_animations.clone()).group(animations_group()));

    let advanced = pages.len();
    pages.push(
        SettingPage::new(tr().settings_advanced.clone())
            .group(duplicates_group())
            .group(advanced_files_group())
            .group(advanced_maintenance_group())
            .group(advanced_reset_group()),
    );

    SettingsPages {
        pages,
        general: general_ix,
        library,
        online: online_ix,
        advanced,
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
    pub advanced: usize,
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

    // The untouched-signal-path toggle: native sample rate through PipeWire.
    // Hidden when this machine cannot offer it at all (no running PipeWire
    // server, or no PipeWire ALSA plugin).
    if audio_output::native_mode_available() {
        let (label, description) = (
            tr().native_rate_button.clone(),
            tr().native_rate_button_desc.clone(),
        );
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
        .item(tab_visibility_item(
            tr().tab_albums.clone(),
            tr().albums_desc.clone(),
            "albums-enabled-toggle",
            |store| store.albums_enabled(),
            |store, on| store.set_albums_enabled(on),
        ))
        .item(tab_visibility_item(
            tr().tab_artists.clone(),
            tr().artists_desc.clone(),
            "artists-enabled-toggle",
            |store| store.artists_enabled(),
            |store, on| store.set_artists_enabled(on),
        ))
        .item(tab_visibility_item(
            tr().tab_folders.clone(),
            tr().folders_desc.clone(),
            "folders-enabled-toggle",
            |store| store.folders_enabled(),
            |store, on| store.set_folders_enabled(on),
        ))
        .item(tab_visibility_item(
            tr().tab_playlists.clone(),
            tr().playlists_desc.clone(),
            "playlists-enabled-toggle",
            |store| store.playlists_enabled(),
            |store, on| store.set_playlists_enabled(on),
        ))
        .item(tab_visibility_item(
            tr().tab_rewind.clone(),
            tr().rewind_desc.clone(),
            "rewind-enabled-toggle",
            |store| store.rewind_enabled(),
            |store, on| store.set_rewind_enabled(on),
        ))
        .description(tr().tabs_all_tracks_note.clone())
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
    // into a phone; `localhost` would be useless for exactly that. When
    // Tailscale is up its address goes underneath, because the phone cannot
    // reach the LAN one once it leaves the house.
    group = group.item(
        SettingItem::new(
            tr().remote_address.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let port = cx.global::<SettingsStore>().remote_port();
                let mut column = v_flex().items_end().gap_1().child(
                    div()
                        .text_sm()
                        .text_color(Colors::muted_foreground(cx))
                        .child(crate::net::remote_url(port)),
                );
                if let Some(url) = crate::net::tailscale_url(port) {
                    column = column.child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(Colors::muted_foreground(cx))
                                    .child(tr().remote_address_tailscale.clone()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(Colors::muted_foreground(cx))
                                    .child(url),
                            ),
                    );
                }
                h_flex().items_center().justify_end().child(column)
            }),
        )
        .description(tr().remote_address_desc.clone()),
    );

    // The code for the phone camera. It encodes the tailnet address when there
    // is one, so a scan keeps working away from home.
    group = group.item(
        SettingItem::new(
            tr().remote_qr.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let show = cx.global::<SettingsStore>().remote_qr();
                h_flex().items_center().justify_end().child(
                    Switch::new("remote-qr-toggle")
                        .checked(show)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) = cx
                                .global_mut::<SettingsStore>()
                                .set_remote_qr(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                        }),
                )
            }),
        )
        .description(tr().remote_qr_desc.clone()),
    );

    group = group.item(
        SettingItem::new(
            tr().remote_qr_code.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let store = cx.global::<SettingsStore>();
                let show = store.remote_qr() && store.remote_enabled();
                let url = crate::net::phone_url(store.remote_port(), Some(store.remote_token()));
                let mut column = v_flex().items_end().gap_2();
                if !show {
                    column = column.child(
                        div()
                            .text_sm()
                            .text_color(Colors::muted_foreground(cx))
                            .child(tr().remote_qr_off.clone()),
                    );
                } else {
                    match crate::remote_qr::path_for(&url) {
                        Some(path) => {
                            column = column.child(
                                img(path)
                                    .w(px(132.))
                                    .h(px(132.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(Colors::border(cx)),
                            );
                        }
                        None => {
                            column = column.child(
                                div()
                                    .text_sm()
                                    .text_color(Colors::muted_foreground(cx))
                                    .child(tr().remote_qr_failed.clone()),
                            );
                        }
                    }
                }
                h_flex().items_center().justify_end().child(column)
            }),
        )
        .description(tr().remote_qr_code_desc.clone()),
    );

    // Off by default: the token is a bearer credential, so it is only worth
    // turning on when the remote is reachable from more than the LAN.
    group = group.item(
        SettingItem::new(
            tr().remote_token.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let token = cx.global::<SettingsStore>().remote_token().to_string();
                let set = !token.is_empty();
                let short: String = token.chars().take(8).collect();
                h_flex()
                    .items_center()
                    .gap_2()
                    .justify_end()
                    .when(set, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(Colors::muted_foreground(cx))
                                .child(if short.is_empty() {
                                    String::new()
                                } else {
                                    format!("{short}\u{2026}")
                                }),
                        )
                    })
                    .child(
                        Button::new("remote-token-button")
                            .small()
                            .label(if set {
                                tr().remote_token_remove.clone()
                            } else {
                                tr().remote_token_generate.clone()
                            })
                            .on_click(|_, _, cx| {
                                let next = if cx.global::<SettingsStore>().remote_token().is_empty()
                                {
                                    crate::net::new_remote_token()
                                } else {
                                    String::new()
                                };
                                if let Err(e) =
                                    cx.global_mut::<SettingsStore>().set_remote_token(&next)
                                {
                                    notify_save_error(cx, e);
                                    return;
                                }
                                // The token is checked by the running server,
                                // so changing it means restarting it.
                                crate::services::apply_remote_state(cx);
                            }),
                    )
            }),
        )
        .description(tr().remote_token_desc.clone()),
    );

    // A phone-UI preference, kept here because this is where the remote is
    // configured; the desktop player is unaffected either way.
    group = group.item(
        SettingItem::new(
            tr().remote_volume_inline.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let inline = cx.global::<SettingsStore>().remote_volume_inline();
                h_flex().items_center().justify_end().child(
                    Switch::new("remote-volume-inline")
                        .checked(inline)
                        .on_click(|new_val, _, cx| {
                            if let Err(e) = cx
                                .global_mut::<SettingsStore>()
                                .set_remote_volume_inline(*new_val)
                            {
                                notify_save_error(cx, e);
                            }
                            crate::services::publish_remote_state(cx);
                        }),
                )
            }),
        )
        .description(tr().remote_volume_inline_desc.clone()),
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
            tr().made_by.clone(),
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
        .title(tr().motion_group.clone())
        .item(
            SettingItem::new(
                tr().animations_play.clone(),
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
            .description(tr().animations_play_desc.clone()),
        );

    // Driven by the same list the call sites use, so a new animation cannot be
    // added to the code and be missing from this page.
    for (ix, kind) in crate::animations::AnimationKind::ALL.into_iter().enumerate() {
        group = group.item(
            SettingItem::new(
                kind.label(),
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
            .description(kind.description()),
        );
    }

    group
}

/// One "show this tab" row.
///
/// There is no row for All Tracks: it is the page every other toggle falls back
/// to, so hiding it would leave the app with nothing to show.
fn tab_visibility_item(
    label: SharedString,
    description: SharedString,
    id: &'static str,
    read: fn(&SettingsStore) -> bool,
    write: fn(&mut SettingsStore, bool) -> anyhow::Result<()>,
) -> SettingItem {
    SettingItem::new(
        label,
        SettingField::render(move |_window, cx: &mut App| {
            let enabled = read(cx.global::<SettingsStore>());
            h_flex().items_center().justify_end().child(
                Switch::new(id)
                    .checked(enabled)
                    .on_click(move |new_val, _, cx| {
                        if let Err(e) = write(cx.global_mut::<SettingsStore>(), *new_val) {
                            notify_save_error(cx, e);
                        }
                    }),
            )
        }),
    )
    .description(description)
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
        K::SettingsOpen => prefs.settings_open,
        K::CoverFade => prefs.cover_fade,
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
        K::SettingsOpen => prefs.settings_open = on,
        K::CoverFade => prefs.cover_fade = on,
    }
}

/// ReplayGain: the one playback preference that changes the sound rather than
/// the interface, so it starts off and says what it does.
fn playback_group() -> SettingGroup {
    use crate::settings_store::ReplayGainMode;
    SettingGroup::new()
        .title(tr().playback_group.clone())
        .item(SettingItem::new(
            tr().replaygain.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let current = cx.global::<SettingsStore>().replaygain();
                let label = match current {
                    ReplayGainMode::Off => tr().replaygain_off.clone(),
                    ReplayGainMode::Track => tr().replaygain_track.clone(),
                    ReplayGainMode::Album => tr().replaygain_album.clone(),
                };
                Button::new("replaygain-dropdown")
                    .small()
                    .label(label)
                    .dropdown_caret(true)
                    .dropdown_menu_with_anchor(Anchor::TopRight, |menu, _, cx| {
                        let current = cx.global::<SettingsStore>().replaygain();
                        ReplayGainMode::ALL
                            .into_iter()
                            .fold(menu, |menu, mode| {
                                let label = match mode {
                                    ReplayGainMode::Off => tr().replaygain_off.clone(),
                                    ReplayGainMode::Track => tr().replaygain_track.clone(),
                                    ReplayGainMode::Album => tr().replaygain_album.clone(),
                                };
                                menu.item(
                                    PopupMenuItem::new(label).checked(mode == current).on_click(
                                        move |_, _, cx| {
                                            if let Err(e) =
                                                cx.global_mut::<SettingsStore>().set_replaygain(mode)
                                            {
                                                notify_save_error(cx, e);
                                            }
                                            // Apply at once to what is playing, rather than
                                            // waiting for the next track change.
                                            crate::services::refresh_track_gain(cx);
                                        },
                                    ),
                                )
                            })
                    })
                    .into_any_element()
            }),
        )
        .description(tr().replaygain_desc.clone()))
        .item(
            SettingItem::new(
                tr().replaygain_clip.clone(),
                SettingField::render(|_window, cx: &mut App| {
                    let on = cx.global::<SettingsStore>().replaygain_prevent_clipping();
                    h_flex().items_center().justify_end().child(
                        Switch::new("replaygain-clip-toggle")
                            .checked(on)
                            .on_click(|new_val, _, cx| {
                                if let Err(e) = cx
                                    .global_mut::<SettingsStore>()
                                    .set_replaygain_prevent_clipping(*new_val)
                                {
                                    notify_save_error(cx, e);
                                }
                                crate::services::refresh_track_gain(cx);
                            }),
                    )
                }),
            )
            .description(tr().replaygain_clip_desc.clone()),
        )
}

fn discord_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().discord_group.clone())
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
        .title(tr().portable_copy.clone())
        .item(
            SettingItem::new(
                tr().portable_include_settings.clone(),
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
            .description(tr().portable_include_settings_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().portable_include_library.clone(),
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
            .description(tr().portable_include_library_desc.clone()),
        )
        .item(
            SettingItem::new(
                tr().portable_export.clone(),
                SettingField::render(|_window, _cx: &mut App| {
                    h_flex().items_center().justify_end().child(
                        Button::new("export-portable")
                            .small()
                            .label(tr().portable_export_button.clone())
                            .on_click(|_, _, cx| start_export(cx)),
                    )
                }),
            )
            .description(tr().portable_desc.clone()),
        )
}

fn start_export(cx: &mut App) {
    let options = crate::portable::ExportOptions {
        settings: cx.global::<SettingsStore>().export_include_settings(),
        library: cx.global::<SettingsStore>().export_include_library(),
    };
    cx.spawn(async move |cx| {
        let Some(handle) = rfd::AsyncFileDialog::new()
            .set_title(tr().portable_choose_folder.clone())
            .pick_folder()
            .await
        else {
            return;
        };
        let destination = handle.path().to_path_buf();
        let result = crate::portable::export(&destination, options);
        cx.update(|cx| {
            let note = match result {
                Ok(root) => Notification::success(tr().portable_exported(&root.display().to_string())),
                Err(err) => Notification::warning(tr().portable_failed(&err.to_string())),
            };
            let note = note.title(tr().portable_copy.clone());
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

/// Everything a user reaches for when something looks wrong: where the files
/// are, how to drop the caches, how to rebuild the catalog, and the one button
/// that puts every preference back to defaults.
///
/// Deliberately not a "developer" page: nothing here needs a terminal, and every
/// action either says what it did or explains why it cannot run.
/// The duplicate finder: an ordered list of folders, and one button that
/// settles every song found in more than one of them.
fn duplicates_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().duplicates_group.clone())
        .item(
            SettingItem::new(
                tr().duplicates_priority.clone(),
                SettingField::render(|_window, cx: &mut App| duplicates_priority_list(cx)),
            )
            .layout(Axis::Vertical),
        )
        .item(
            SettingItem::new(
                tr().duplicates_find.clone(),
                SettingField::render(|_window, _cx: &mut App| {
                    h_flex().items_center().justify_end().child(
                        Button::new("find-duplicates")
                            .small()
                            .label(tr().duplicates_find.clone())
                            .on_click(|_, window, cx| find_duplicates(window, cx)),
                    )
                }),
            )
            .description(tr().duplicates_desc.clone()),
        )
}

/// The folder list, with one row per folder and the arrows that reorder it.
fn duplicates_priority_list(cx: &App) -> AnyElement {
    let priority = cx.global::<SettingsStore>().duplicate_priority().to_vec();
    let muted = Colors::muted_foreground(cx);

    if priority.is_empty() {
        return v_flex()
            .gap_2()
            .child(div().text_sm().text_color(muted).child(tr().duplicates_empty.clone()))
            .child(
                h_flex()
                    .gap_2()
                    .child(duplicates_add_button("duplicates-seed-empty"))
                    .child(duplicates_seed_button("duplicates-seed-empty-2")),
            )
            .into_any_element();
    }

    let count = priority.len();
    let mut rows = v_flex().gap_1();
    for (index, folder) in priority.iter().enumerate() {
        let name = std::path::Path::new(folder)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.clone());
        let can_up = index > 0;
        let can_down = index + 1 < count;

        let mut move_up = Button::new(("duplicates-up", index))
            .small()
            .tooltip(tr().duplicates_up.clone())
            .label("↑");
        if !can_up {
            move_up = move_up.disabled(true);
        }
        let mut move_down = Button::new(("duplicates-down", index))
            .small()
            .tooltip(tr().duplicates_down.clone())
            .label("↓");
        if !can_down {
            move_down = move_down.disabled(true);
        }

        rows = rows.child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(20.))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(muted)
                        .child(SharedString::from(format!("{}", index + 1))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_sm()
                        .child(SharedString::from(name)),
                )
                .child(move_up.on_click(move |_, _, cx| move_priority(index, index - 1, cx)))
                .child(move_down.on_click(move |_, _, cx| move_priority(index, index + 1, cx)))
                .child(
                    Button::new(("duplicates-remove", index))
                        .small()
                        .label("✕")
                        .on_click(move |_, _, cx| {
                            let mut list =
                                cx.global::<SettingsStore>().duplicate_priority().to_vec();
                            list.remove(index);
                            if let Err(err) =
                                cx.global_mut::<SettingsStore>().set_duplicate_priority(list)
                            {
                                notify_save_error(cx, err);
                            }
                        }),
                ),
        );
    }

    rows.child(
        h_flex()
            .gap_2()
            .pt_1()
            .child(duplicates_add_button("duplicates-add"))
            .child(duplicates_seed_button("duplicates-seed")),
    )
    .into_any_element()
}

fn duplicates_add_button(id: &'static str) -> Button {
    Button::new(id)
        .small()
        .label(tr().duplicates_add_folder.clone())
        .on_click(|_, _, cx| {
            // Same picker the library folders use, and the same reason it is
            // async: a blocking dialog on the UI thread freezes the window.
            let task = rfd::AsyncFileDialog::new().pick_folder();
            cx.spawn(async move |cx| {
                if let Some(handle) = task.await {
                    let path = handle.path().to_string_lossy().into_owned();
                    cx.update(|cx| {
                        let mut list = cx.global::<SettingsStore>().duplicate_priority().to_vec();
                        if !list.contains(&path) {
                            list.push(path);
                            if let Err(err) =
                                cx.global_mut::<SettingsStore>().set_duplicate_priority(list)
                            {
                                notify_save_error(cx, err);
                            }
                        }
                    });
                }
            })
            .detach();
        })
}

/// Fill the list from the library's own folders, so the first use is one click.
fn duplicates_seed_button(id: &'static str) -> Button {
    Button::new(id)
        .small()
        .label(tr().duplicates_seed.clone())
        .on_click(|_, _, cx| {
            let library = cx.global::<Services>().library.clone();
            let mut seeded: Vec<String> = Vec::new();
            for root in &library.folder_roots() {
                for child in library.child_folders(&root.path) {
                    if !seeded.contains(&child.path) {
                        seeded.push(child.path);
                    }
                }
            }
            if seeded.is_empty() {
                // A library of bare folders has no subfolders to offer; the
                // roots themselves are still worth listing.
                seeded = library
                    .folder_roots()
                    .into_iter()
                    .map(|root| root.path)
                    .collect();
            }
            if let Err(err) = cx
                .global_mut::<SettingsStore>()
                .set_duplicate_priority(seeded)
            {
                notify_save_error(cx, err);
            }
        })
}

fn move_priority(from: usize, to: usize, cx: &mut App) {
    let mut list = cx.global::<SettingsStore>().duplicate_priority().to_vec();
    if from >= list.len() || to >= list.len() {
        return;
    }
    let item = list.remove(from);
    list.insert(to, item);
    if let Err(err) = cx
        .global_mut::<SettingsStore>()
        .set_duplicate_priority(list)
    {
        notify_save_error(cx, err);
    }
}

fn find_duplicates(window: &mut Window, cx: &mut App) {
    let priority = cx.global::<SettingsStore>().duplicate_priority().to_vec();
    if priority.is_empty() {
        window.push_notification(
            Notification::warning(tr().duplicates_empty.clone())
                .title(tr().duplicates_group.clone()),
            cx,
        );
        return;
    }

    let report = crate::duplicates::find(
        crate::duplicates::candidates(&cx.global::<Services>().library),
        &priority,
    );

    if report.is_empty() {
        window.push_notification(
            Notification::success(tr().duplicates_none.clone()).title(tr().duplicates_found.clone()),
            cx,
        );
        return;
    }

    let paths: Vec<String> = report.removable.iter().map(|file| file.path.clone()).collect();
    let summary = SharedString::from(format!(
        "{} · {} files · {}",
        tr().selection_count(report.duplicates.len()),
        report.removable.len(),
        tr().size(report.removable_bytes())
    ));
    let sample: Vec<SharedString> = report
        .removable
        .iter()
        .take(12)
        .map(|file| SharedString::from(format!("{} — {}", file.title, file.path)))
        .collect();
    let more = report.removable.len().saturating_sub(sample.len());

    window.open_dialog(cx, move |dialog, _, dialog_cx| {
        let paths = paths.clone();
        let muted = Colors::muted_foreground(dialog_cx);
        let mut lines = v_flex().gap_1().w_full();
        for line in &sample {
            lines = lines.child(
                div()
                    .text_xs()
                    .truncate()
                    .text_color(muted)
                    .child(line.clone()),
            );
        }
        if more > 0 {
            lines = lines.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(tr().duplicates_more(more))),
            );
        }
        dialog
            .title(tr().duplicates_confirm_title.clone())
            .w(px(640.))
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().child(summary.clone()))
                    .child(div().text_sm().child(tr().duplicates_confirm_body.clone()))
                    .child(lines),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("duplicates-cancel")
                            .label(tr().cancel.clone())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("duplicates-confirm")
                            .primary()
                            .label(tr().duplicates_move_to_trash.clone())
                            .on_click(move |_, window, cx| {
                                trash_duplicates(paths.clone(), window, cx);
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

fn trash_duplicates(paths: Vec<String>, window: &mut Window, cx: &mut App) {
    let (moved, failures) = crate::duplicates::trash(&paths);
    if moved > 0 {
        window.push_notification(
            Notification::success(tr().duplicates_moved(moved))
                .title(tr().duplicates_group.clone()),
            cx,
        );
    }
    if !failures.is_empty() {
        log::warn!("duplicates: {} files could not be trashed", failures.len());
        window.push_notification(
            Notification::warning(tr().duplicates_failed(failures.len()))
                .title(tr().duplicates_group.clone()),
            cx,
        );
    }
    // The catalog still lists the files that moved; a scan retires them.
    let folders = cx.global::<SettingsStore>().music_folders().to_vec();
    if !folders.is_empty() {
        cx.global::<Services>().library.clear_and_rescan(folders);
    }
}

fn advanced_files_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().advanced_files.clone())
        .item(SettingItem::new(
            tr().advanced_data_dir.clone(),
            SettingField::render(|_window, cx: &mut App| {
                reveal_row(crate::paths::data_dir(), "reveal-data-dir", cx)
            }),
        ))
        .item(SettingItem::new(
            tr().advanced_config_dir.clone(),
            SettingField::render(|_window, cx: &mut App| {
                reveal_row(crate::paths::config_dir(), "reveal-config-dir", cx)
            }),
        ))
        .item(SettingItem::new(
            tr().advanced_logs_dir.clone(),
            SettingField::render(|_window, cx: &mut App| {
                let logs = crate::paths::data_dir().map(|dir| dir.join("logs"));
                reveal_row(logs, "reveal-logs-dir", cx)
            }),
        ))
}

fn advanced_maintenance_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().advanced_maintenance.clone())
        .item(action_row(
            tr().advanced_clear_covers.clone(),
            "clear-cover-cache",
            tr().advanced_clear_covers_desc.clone(),
            clear_cover_caches,
        ))
        .item(action_row(
            tr().advanced_rescan.clone(),
            "rebuild-library",
            tr().advanced_rescan_desc.clone(),
            rebuild_library,
        ))
        .item(action_row(
            tr().advanced_vacuum.clone(),
            "vacuum-database",
            tr().advanced_vacuum_desc.clone(),
            compact_database,
        ))
        .item(action_row(
            tr().advanced_reset_window.clone(),
            "reset-window-size",
            tr().advanced_reset_window_desc.clone(),
            reset_window_size,
        ))
}

fn advanced_reset_group() -> SettingGroup {
    SettingGroup::new()
        .title(tr().advanced_danger.clone())
        .item(SettingItem::new(
            tr().advanced_reset_all.clone(),
            SettingField::render(|_window, _cx: &mut App| {
                h_flex().items_center().justify_end().child(
                    Button::new("reset-all-settings")
                        .small()
                        .label(tr().advanced_reset_all.clone())
                        .on_click(|_, window, cx| open_reset_settings_dialog(window, cx)),
                )
            }),
        )
        .description(tr().advanced_reset_all_desc.clone()))
}

/// A read-only path with a button that opens it in the file manager.
fn reveal_row(
    path: Option<std::path::PathBuf>,
    id: &'static str,
    cx: &App,
) -> AnyElement {
    let muted = Colors::muted_foreground(cx);
    let label: SharedString = match &path {
        Some(path) => path.display().to_string().into(),
        None => tr().not_available.clone(),
    };
    h_flex()
        .items_center()
        .justify_end()
        .gap_2()
        .child(
            div()
                .max_w(px(340.))
                .overflow_hidden()
                .text_ellipsis()
                .text_sm()
                .text_color(muted)
                .child(label),
        )
        .child(
            Button::new(id)
                .small()
                .label(tr().reveal_folder.clone())
                .disabled(path.is_none())
                .on_click(move |_, _, _| {
                    if let Some(path) = &path {
                        reveal_in_file_manager(path);
                    }
                }),
        )
        .into_any_element()
}

/// One row whose field is a single button, with its explanation underneath.
fn action_row(
    label: SharedString,
    id: &'static str,
    description: SharedString,
    run: fn(&mut Window, &mut App),
) -> SettingItem {
    SettingItem::new(
        label.clone(),
        SettingField::render(move |_window, _cx: &mut App| {
            h_flex().items_center().justify_end().child(
                Button::new(id)
                    .small()
                    .label(label.clone())
                    .on_click(move |_, window, cx| run(window, cx)),
            )
        }),
    )
    .description(description)
}

fn clear_cover_caches(window: &mut Window, cx: &mut App) {
    let cache = cx.global::<Services>().cover_art_cache.clone();
    let memory = cache.borrow().small_len();
    cache.borrow_mut().clear(cx);
    // The artwork directory is what MPRIS/Discord hand to the desktop shell; it
    // is regenerated on the next track, so clearing it is always safe.
    let mut removed = 0usize;
    let artwork = crate::paths::cache_dir().join("artwork");
    if let Ok(entries) = std::fs::read_dir(&artwork) {
        for entry in entries.flatten() {
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    window.push_notification(
        Notification::success(tr().advanced_clear_covers_done(memory, removed))
            .title(tr().advanced_clear_covers.clone()),
        cx,
    );
}

fn rebuild_library(window: &mut Window, cx: &mut App) {
    let folders = cx
        .global::<SettingsStore>()
        .music_folders()
        .to_vec();
    if folders.is_empty() {
        window.push_notification(
            Notification::warning(tr().no_music_sources.clone())
                .title(tr().advanced_rescan.clone()),
            cx,
        );
        return;
    }
    cx.global::<Services>().library.clear_and_rescan(folders);
    window.push_notification(
        Notification::success(tr().advanced_rescan_started.clone())
            .title(tr().advanced_rescan.clone()),
        cx,
    );
}

fn compact_database(window: &mut Window, cx: &mut App) {
    // VACUUM needs the write lock for its whole run; during a scan that means a
    // busy timeout and a failure the user cannot act on.
    if cx.global::<Services>().library.is_scanning() {
        window.push_notification(
            Notification::warning(tr().scanning.clone()).title(tr().advanced_vacuum.clone()),
            cx,
        );
        return;
    }
    let result = cx.global::<Services>().library.repo().vacuum();
    let notification = match result {
        Ok(()) => Notification::success(tr().advanced_vacuum_done.clone()),
        Err(err) => Notification::error(format!("{err}")),
    };
    window.push_notification(notification.title(tr().advanced_vacuum.clone()), cx);
}

fn reset_window_size(window: &mut Window, cx: &mut App) {
    let saved = cx.global_mut::<SettingsStore>().reset_window_size();
    match saved {
        Ok(()) => window.push_notification(
            Notification::success(tr().advanced_reset_window_done.clone())
                .title(tr().advanced_reset_window.clone()),
            cx,
        ),
        Err(err) => notify_save_error(cx, err),
    }
}

fn open_reset_settings_dialog(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog
            .title(tr().advanced_reset_all_confirm.clone())
            .w(px(460.))
            .child(div().child(tr().advanced_reset_all_body.clone()))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("reset-cancel")
                            .label(tr().cancel.clone())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("reset-confirm")
                            .primary()
                            .label(tr().advanced_reset_all.clone())
                            .on_click(|_, window, cx| {
                                reset_all_settings(window, cx);
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

fn reset_all_settings(window: &mut Window, cx: &mut App) {
    match cx.global_mut::<SettingsStore>().reset_preferences() {
        Ok(()) => {
            // A reset has to *apply*, not just persist: several settings drive
            // things that were set up once at startup, and leaving them live
            // would make the app disagree with its own settings file until the
            // next launch.
            let font_scale = cx.global::<SettingsStore>().font_scale();
            crate::settings_store::apply_base_theme(cx);
            crate::settings_store::apply_font_scale(font_scale, cx);
            // Remote and Rich Presence default to off, so a reset has to stop
            // them as well as forget them.
            crate::services::apply_remote_state(cx);
            crate::discord_bridge::set_enabled(cx, false);
            // The queue is live state and is about to be written back on the
            // next track change, so put it into the freshly reset settings now
            // rather than leaving a default block that would be overwritten.
            crate::services::save_playback(cx);
            window.push_notification(
            Notification::success(tr().advanced_reset_all_done.clone())
                .title(tr().advanced_reset_all.clone()),
            cx,
        );
        }
        Err(err) => notify_save_error(cx, err),
    }
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
    // The saved value can point at a tab that is now hidden. Resolve it the way
    // the app will at launch, then map that back to the first offered entry it
    // matches — so the checked row is always one that actually opens.
    use crate::library_views::library_view::LibraryRootTab;
    let resolved = LibraryRootTab::resolve_startup(store.startup_tab(), store);
    let current = [
        StartupTab::Albums,
        StartupTab::Artists,
        StartupTab::AllTracks,
        StartupTab::Folders,
        StartupTab::Playlists,
    ]
    .into_iter()
    .find(|tab| LibraryRootTab::from_startup(*tab) == resolved)
    .unwrap_or(StartupTab::AllTracks);
    let offered = [
        (StartupTab::Albums, tr().tab_albums.clone()),
        (StartupTab::Artists, tr().tab_artists.clone()),
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
        // Reported as Albums by `SettingsStore::startup_tab`, so it reads as one.
        StartupTab::Genres => tr().tab_albums.clone(),
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
