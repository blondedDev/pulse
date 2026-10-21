use gpui::{
    AnyElement, App, Axis, Entity, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputState, NumberInput},
    slider::Slider,
    switch::Switch,
    time_field::TimeField,
    v_flex,
};
use ui_components::settings::{SettingField, SettingGroup, SettingItem};
use ui_resources::i18n::tools_strings;

use crate::settings_store::{
    SLEEP_TIMER_DURATIONS, SettingsStore, SleepTimerSettings, notify_save_error,
};
use crate::theme_colors::Colors;

use super::controls::SleepTimerControls;
use super::{Armed, EXTEND_MIN, SleepTimer, clock_label};

pub const SETTINGS_ANCHOR: &str = "sleep-timer";
const DURATION_INPUT_WIDTH: f32 = 112.;
const FADE_SLIDER_WIDTH: f32 = 160.;
const FADE_LABEL_MIN_WIDTH: f32 = 48.;

pub(super) fn update(cx: &mut App, change: impl FnOnce(&mut SleepTimerSettings)) {
    let mut settings = cx.global::<SettingsStore>().sleep_timer();
    change(&mut settings);
    if let Err(e) = cx.global_mut::<SettingsStore>().set_sleep_timer(settings) {
        notify_save_error(cx, e);
    }
}

pub fn duration_field(input: &Entity<InputState>, disabled: bool, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(DURATION_INPUT_WIDTH))
                .child(NumberInput::new(input).small().disabled(disabled)),
        )
        .child(
            div()
                .text_sm()
                .text_color(Colors::muted_foreground(cx))
                .child(tools_strings().timer_minutes_unit.clone()),
        )
        .into_any_element()
}

pub fn fade_item(id: &'static str, controls: Entity<SleepTimerControls>) -> SettingItem {
    SettingItem::new(
        tools_strings().timer_fade.clone(),
        SettingField::render(move |_window, cx: &mut App| {
            let enabled = cx.global::<SettingsStore>().sleep_timer().fade_out;
            let (slider, label) = {
                let controls = controls.read(cx);
                (controls.fade.clone(), controls.fade_label())
            };
            h_flex()
                .gap_3()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .min_w(px(FADE_LABEL_MIN_WIDTH))
                        .text_right()
                        .text_sm()
                        .text_color(Colors::muted_foreground(cx))
                        .child(label),
                )
                .child(
                    div()
                        .w(px(FADE_SLIDER_WIDTH))
                        .child(Slider::new(&slider).disabled(!enabled)),
                )
                .child(
                    Switch::new(id)
                        .checked(enabled)
                        .on_click(|on, _, cx| update(cx, |s| s.fade_out = *on)),
                )
        }),
    )
    .description(tools_strings().timer_fade_desc.clone())
}

fn auto_item(controls: Entity<SleepTimerControls>) -> SettingItem {
    SettingItem::new(
        tools_strings().timer_auto.clone(),
        SettingField::render(move |_window, cx: &mut App| {
            let enabled = cx.global::<SettingsStore>().sleep_timer().auto;
            let (from, until) = {
                let controls = controls.read(cx);
                (controls.from.clone(), controls.until.clone())
            };
            h_flex()
                .gap_2()
                .items_center()
                .justify_end()
                .child(TimeField::new(&from).small().disabled(!enabled))
                .child(
                    div()
                        .text_sm()
                        .text_color(Colors::muted_foreground(cx))
                        .child("–"),
                )
                .child(TimeField::new(&until).small().disabled(!enabled))
                .child(
                    div().pl_2().child(
                        Switch::new("sleep-timer-auto")
                            .checked(enabled)
                            .on_click(|on, _, cx| update(cx, |s| s.auto = *on)),
                    ),
                )
        }),
    )
    .description(tools_strings().timer_auto_desc.clone())
}

fn duration_item(controls: Entity<SleepTimerControls>) -> SettingItem {
    SettingItem::new(
        tools_strings().timer_auto_duration.clone(),
        SettingField::render(move |_window, cx: &mut App| {
            let enabled = cx.global::<SettingsStore>().sleep_timer().auto;
            let input = controls.read(cx).auto_duration.clone();
            duration_field(&input, !enabled, cx)
        }),
    )
}

fn status_field(timer: Entity<SleepTimer>, cx: &mut App) -> AnyElement {
    let s = tools_strings();
    let (armed, status) = {
        let t = timer.read(cx);
        (t.armed(), t.status())
    };
    let mut row = h_flex().gap_2().items_center().child(
        div()
            .flex_1()
            .text_sm()
            .text_color(if armed.is_some() {
                Colors::foreground(cx)
            } else {
                Colors::muted_foreground(cx)
            })
            .child(status),
    );
    if matches!(armed, Some(Armed::Until { .. })) {
        let timer = timer.clone();
        row = row.child(
            Button::new("sleep-timer-extend")
                .small()
                .label(SharedString::from(s.timer_extend(EXTEND_MIN)))
                .on_click(move |_, _, cx| timer.update(cx, |t, cx| t.extend(EXTEND_MIN, cx))),
        );
    }
    if armed.is_some() {
        row = row.child(
            Button::new("sleep-timer-cancel")
                .small()
                .label(s.timer_cancel.clone())
                .on_click(move |_, _, cx| timer.update(cx, |t, cx| t.cancel(cx))),
        );
    }
    row.into_any_element()
}

fn presets_field(
    timer: Entity<SleepTimer>,
    controls: Entity<SleepTimerControls>,
    cx: &mut App,
) -> AnyElement {
    let s = tools_strings();
    let end_of_track = timer.read(cx).armed() == Some(Armed::EndOfTrack);
    let mut row = h_flex().gap_2().flex_wrap();
    for (ix, minutes) in SLEEP_TIMER_DURATIONS.into_iter().enumerate() {
        let timer = timer.clone();
        row = row.child(
            Button::new(("sleep-timer-preset", ix))
                .small()
                .label(SharedString::from(s.timer_minutes(minutes)))
                .on_click(move |_, _, cx| timer.update(cx, |t, cx| t.start(minutes, cx))),
        );
    }
    let end_timer = timer.clone();
    row = row.child(
        Button::new("sleep-timer-end-of-track")
            .small()
            .label(s.timer_end_of_track.clone())
            .when(end_of_track, |b| b.primary())
            .on_click(move |_, _, cx| end_timer.update(cx, |t, cx| t.start_end_of_track(cx))),
    );
    let input = controls.read(cx).manual_duration.clone();
    let custom = h_flex()
        .gap_2()
        .items_center()
        .child(duration_field(&input, false, cx))
        .child(
            Button::new("sleep-timer-custom-start")
                .small()
                .label(s.timer_run.clone())
                .on_click(move |_, _, cx| {
                    let minutes = controls.read(cx).manual_minutes();
                    timer.update(cx, |t, cx| t.start(minutes, cx));
                }),
        );
    v_flex().gap_2().child(row).child(custom).into_any_element()
}

fn auto_status_field(cx: &mut App) -> AnyElement {
    let settings = cx.global::<SettingsStore>().sleep_timer();
    let state = if settings.auto {
        SharedString::from(format!(
            "{}–{}",
            clock_label(settings.auto_from_min),
            clock_label(settings.auto_until_min)
        ))
    } else {
        tools_strings().timer_off.clone()
    };
    h_flex()
        .gap_2()
        .items_center()
        .justify_end()
        .child(
            div()
                .text_sm()
                .text_color(Colors::muted_foreground(cx))
                .child(state),
        )
        .into_any_element()
}

pub fn settings_group(cx: &App, controls: Entity<SleepTimerControls>) -> SettingGroup {
    settings_group_with_timer(super::timer(cx), controls)
}

/// The Sleep Timer group for the Settings screen.
///
/// This absorbs what used to be the Tools > Timer page: live status with
/// extend/cancel, the quick presets, the custom duration, and the auto window
/// summary all live here now, so removing Tools loses nothing.
pub fn settings_group_with_timer(
    timer: Option<Entity<SleepTimer>>,
    controls: Entity<SleepTimerControls>,
) -> SettingGroup {
    let s = tools_strings();
    let mut group = SettingGroup::new()
        .anchor(SETTINGS_ANCHOR)
        .title(s.tools_timer.clone());
    if let Some(timer) = timer {
        let status_timer = timer.clone();
        let presets_timer = timer.clone();
        let presets_controls = controls.clone();
        group = group
            .item(SettingItem::unlabeled(SettingField::render(
                move |_window, cx: &mut App| status_field(status_timer.clone(), cx),
            )))
            .item(
                SettingItem::new(
                    s.timer_start.clone(),
                    SettingField::render(move |_window, cx: &mut App| {
                        presets_field(presets_timer.clone(), presets_controls.clone(), cx)
                    }),
                )
                .layout(Axis::Vertical),
            );
    }
    group
        .item(SettingItem::new(
            s.timer_auto.clone(),
            SettingField::render(|_window, cx: &mut App| auto_status_field(cx)),
        ))
        .item(auto_item(controls.clone()))
        .item(duration_item(controls.clone()))
        .item(fade_item("sleep-timer-fade-general", controls))
}
