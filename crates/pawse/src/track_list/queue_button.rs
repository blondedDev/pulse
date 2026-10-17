use std::rc::Rc;

use gpui::{
    App, ElementId, IntoElement, ParentElement, StatefulInteractiveElement, Window, div, px,
};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::{Cancel, Confirm, DialogFooter},
    notification::Notification,
    tooltip::Tooltip,
};
use music_library::Track;

use crate::theme_colors::Colors;

use super::{RowButtonColors, row_icon_button};
use crate::localization::tr;
use crate::playback_queue::QueueSource;
use crate::services::Services;
use crate::settings_store::SettingsStore;

pub fn append_tracks_to_queue(tracks: Vec<Rc<Track>>, cx: &mut App) {
    if tracks.is_empty() {
        return;
    }
    let dedup = cx.global::<SettingsStore>().queue_deduplication();
    cx.global::<Services>()
        .playback_queue
        .borrow_mut()
        .append_tracks(tracks, dedup);
    crate::services::queue_mutated(cx);
}

/// Queue tracks to play immediately after the current one — "play next", not
/// "add to the end". Returns how many actually landed, because deduplication can
/// drop some and the caller must not toast about a track that was skipped.
pub fn play_tracks_next(tracks: Vec<Rc<Track>>, cx: &mut App) -> usize {
    if tracks.is_empty() {
        return 0;
    }
    let dedup = cx.global::<SettingsStore>().queue_deduplication();
    let inserted = cx
        .global::<Services>()
        .playback_queue
        .borrow_mut()
        .insert_next(tracks, dedup);
    if inserted > 0 {
        crate::services::queue_mutated(cx);
    }
    inserted
}

/// The confirmation for a play-next. Shared by the row button and the `Q`
/// shortcut so both say the same thing.
pub fn toast_playing_next(window: &mut Window, title: &str, cx: &mut App) {
    window.push_notification(
        Notification::success(tr().playing_next(title)).title(tr().play_next.clone()),
        cx,
    );
}

pub fn replace_queue_and_play(
    tracks: Vec<Rc<Track>>,
    index: usize,
    source: QueueSource,
    cx: &mut App,
) {
    let services = cx.global::<Services>();
    let track = services
        .playback_queue
        .borrow_mut()
        .set_tracks_and_play_at(tracks, index, source)
        .cloned();
    if let Some(track) = track {
        services.play_track(&track);
        crate::services::save_playback(cx);
    }
}

pub fn play_replacing_queue(
    tracks: Vec<Rc<Track>>,
    index: usize,
    source: QueueSource,
    window: &mut Window,
    cx: &mut App,
) {
    if tracks.get(index).is_some_and(|track| !track.available) {
        return;
    }
    if !cx.global::<Services>().playback_queue.borrow().is_custom() {
        replace_queue_and_play(tracks, index, source, cx);
        return;
    }
    let tracks = Rc::new(tracks);
    let clicked = tracks.get(index).cloned();
    window.open_dialog(cx, move |dialog, _, _| {
        let tracks = tracks.clone();
        let clicked = clicked.clone();
        dialog
            .overlay_closable(false)
            .close_button(false)
            .w(px(640.))
            .title(tr().replace_queue_confirm_title.clone())
            .child(div().child(tr().replace_queue_confirm_message.clone()))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("dialog-add-to-queue")
                            .label(tr().add_to_queue.clone())
                            .on_click(move |_, window, cx| {
                                if let Some(track) = clicked.clone() {
                                    append_tracks_to_queue(vec![track], cx);
                                }
                                window.close_dialog(cx);
                            }),
                    )
                    .child(
                        Button::new("cancel")
                            .label(tr().cancel.clone())
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(Cancel), cx)),
                    )
                    .child(
                        Button::new("ok")
                            .label(tr().replace_queue.clone())
                            .primary()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(Confirm { secondary: false }), cx)
                            }),
                    ),
            )
            .on_ok(move |_, _, cx| {
                replace_queue_and_play((*tracks).clone(), index, source, cx);
                true
            })
    });
}

pub fn play_next_button(
    track: Rc<Track>,
    button_size: f32,
    icon_size: f32,
    colors: &RowButtonColors,
) -> impl IntoElement {
    let title = track.title.clone();
    row_icon_button(
        ElementId::NamedInteger("play-next".into(), track.id as u64),
        button_size,
        "icons/s1-play-next.svg",
        icon_size,
        colors.icon,
        colors.icon_hover,
        true,
    )
    .tooltip(|window, cx| Tooltip::new(tr().play_next.clone()).build(window, cx))
    .on_click(move |_, window, cx| {
        cx.stop_propagation();
        if play_tracks_next(vec![track.clone()], cx) > 0 {
            toast_playing_next(window, &title, cx);
        }
    })
}

pub fn add_to_queue_button(
    track: Rc<Track>,
    button_size: f32,
    icon_size: f32,
    colors: &RowButtonColors,
) -> impl IntoElement {
    row_icon_button(
        ElementId::NamedInteger("add-to-queue".into(), track.id as u64),
        button_size,
        "icons/add-queue.svg",
        icon_size,
        colors.icon,
        colors.icon_hover,
        true,
    )
    .tooltip(|window, cx| Tooltip::new(tr().add_to_queue.clone()).build(window, cx))
    .on_click(move |_, _, cx| {
        cx.stop_propagation();
        append_tracks_to_queue(vec![track.clone()], cx);
    })
}

pub fn add_album_to_queue_button(
    album_id: i64,
    button_size: f32,
    icon_size: f32,
    cx: &App,
) -> impl IntoElement {
    row_icon_button(
        ElementId::NamedInteger("add-album-to-queue".into(), album_id as u64),
        button_size,
        "icons/add-queue.svg",
        icon_size,
        Colors::muted_foreground(cx),
        Colors::muted(cx),
        false,
    )
    .tooltip(|window, cx| Tooltip::new(tr().add_album_to_queue.clone()).build(window, cx))
    .on_click(move |_, _, cx| {
        cx.stop_propagation();
        append_album_to_queue(album_id, cx);
    })
}

pub fn append_album_to_queue(album_id: i64, cx: &mut App) {
    let tracks = cx.global::<Services>().library.tracks_for_album(album_id);
    append_tracks_to_queue(tracks.into_iter().map(Rc::new).collect(), cx);
}
