//! Motion, in one place.
//!
//! Every animation the app can play is listed in [`AnimationKind`], and every
//! call site asks this module whether to animate rather than deciding for
//! itself. Adding a kind is: a variant, a duration, and one call to [`fade_in`]
//! or [`rise_in`].
//!
//! The rule everything here obeys: an animation may change how something looks
//! while it settles, never whether it exists, where it is, or whether it can be
//! clicked. Turning them off must leave the app exactly as usable as it is with
//! them on — which is why every path has a non-animated branch rather than a
//! zero-length animated one.

use std::time::Duration;

use gpui::{px, Animation, AnimationExt, AnyElement, App, Div, ElementId, IntoElement, Styled};
use gpui::ease_out_quint;

use crate::settings_store::{AnimationSettings, SettingsStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationKind {
    /// The queue and lyrics panels sliding open and closed.
    PanelSlide,
    /// Library content fading in when the tab or the page changes.
    ViewFade,
    /// The now-playing bar settling in when the track changes.
    TrackChange,
    /// The cover view drifting up as it appears, and again between tracks.
    CoverMode,
    /// The Settings screen rising into place as it opens.
    SettingsOpen,
    /// Album covers fading in as they finish loading off the UI thread.
    CoverFade,
}

impl AnimationKind {
    pub const ALL: [Self; 6] = [
        Self::PanelSlide,
        Self::ViewFade,
        Self::TrackChange,
        Self::CoverMode,
        Self::SettingsOpen,
        Self::CoverFade,
    ];

    pub fn label(self) -> gpui::SharedString {
        let s = crate::localization::tr();
        match self {
            Self::PanelSlide => s.animation_panel_slide.clone(),
            Self::ViewFade => s.animation_view_fade.clone(),
            Self::TrackChange => s.animation_track_change.clone(),
            Self::CoverMode => s.animation_cover_mode.clone(),
            Self::SettingsOpen => s.animation_settings_open.clone(),
            Self::CoverFade => s.animation_cover_fade.clone(),
        }
    }

    pub fn description(self) -> gpui::SharedString {
        let s = crate::localization::tr();
        match self {
            Self::PanelSlide => s.animation_panel_slide_desc.clone(),
            Self::ViewFade => s.animation_view_fade_desc.clone(),
            Self::TrackChange => s.animation_track_change_desc.clone(),
            Self::CoverMode => s.animation_cover_mode_desc.clone(),
            Self::SettingsOpen => s.animation_settings_open_desc.clone(),
            Self::CoverFade => s.animation_cover_fade_desc.clone(),
        }
    }

    fn duration(self) -> Duration {
        match self {
            // Matches the timer that drives the panel closing state machine.
            Self::PanelSlide => Duration::from_millis(200),
            Self::ViewFade => Duration::from_millis(140),
            Self::TrackChange => Duration::from_millis(180),
            Self::CoverMode => Duration::from_millis(240),
            // The settings screen is a whole view: long enough to read as a
            // transition, short enough not to sit between a click and its
            // content.
            Self::SettingsOpen => Duration::from_millis(200),
            // Covers land one at a time from the background executor, so this
            // is a settle, not a reveal.
            Self::CoverFade => Duration::from_millis(160),
        }
    }

    fn switched_on(self, prefs: AnimationSettings) -> bool {
        match self {
            Self::PanelSlide => prefs.panel_slide,
            Self::ViewFade => prefs.view_fade,
            Self::TrackChange => prefs.track_change,
            Self::CoverMode => prefs.cover_mode,
            Self::SettingsOpen => prefs.settings_open,
            Self::CoverFade => prefs.cover_fade,
        }
    }
}

/// How long this animation should run, or `None` when it should not run at all.
pub fn duration(kind: AnimationKind, cx: &App) -> Option<Duration> {
    let prefs = cx.global::<SettingsStore>().animations();
    (prefs.enabled && kind.switched_on(prefs)).then(|| kind.duration())
}

/// Duration for the panel slide, falling back to `ZERO` when it is off.
///
/// The panels animate their own width, so "off" has to be expressible as a
/// duration: the element still renders, it just reaches its final width on the
/// first frame.
pub fn panel_slide(cx: &App) -> Duration {
    duration(AnimationKind::PanelSlide, cx).unwrap_or(Duration::ZERO)
}

/// How long a cover should fade in for, or `None` when it should just appear.
pub fn cover_fade(cx: &App) -> Option<Duration> {
    duration(AnimationKind::CoverFade, cx)
}

/// Fade `element` in whenever `id` changes.
///
/// `id` is the trigger: a new id restarts the fade, the same id leaves the
/// element alone. Pass something that identifies what the user is looking at
/// (a tab, a track), not a per-frame counter, or it will animate forever.
pub fn fade_in(kind: AnimationKind, id: impl Into<ElementId>, element: Div, cx: &App) -> AnyElement {
    match duration(kind, cx) {
        Some(ms) => element
            .with_animation(id, Animation::new(ms).with_easing(ease_out_quint()), |el, delta| {
                el.opacity(delta)
            })
            .into_any_element(),
        None => element.into_any_element(),
    }
}

/// Fade in while drifting up by `distance`.
///
/// The drift is a `top` offset, so it never changes layout: the element occupies
/// its final space from the first frame and only its paint position moves.
pub fn rise_in(
    kind: AnimationKind,
    id: impl Into<ElementId>,
    element: Div,
    distance: f32,
    cx: &App,
) -> AnyElement {
    match duration(kind, cx) {
        Some(ms) => element
            .relative()
            .with_animation(id, Animation::new(ms).with_easing(ease_out_quint()), move |el, delta| {
                el.opacity(delta).top(px(distance * (1.0 - delta)))
            })
            .into_any_element(),
        None => element.into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_on() -> AnimationSettings {
        AnimationSettings::default()
    }

    #[test]
    fn every_kind_is_on_by_default() {
        for kind in AnimationKind::ALL {
            assert!(kind.switched_on(all_on()), "{kind:?} should start on");
        }
    }

    #[test]
    fn each_kind_has_its_own_switch() {
        let only_fade = AnimationSettings {
            panel_slide: false,
            ..all_on()
        };
        assert!(AnimationKind::ViewFade.switched_on(only_fade));
        assert!(!AnimationKind::PanelSlide.switched_on(only_fade));
        assert!(AnimationKind::TrackChange.switched_on(only_fade));
    }

    #[test]
    fn every_kind_has_a_visible_label_and_a_positive_duration() {
        for kind in AnimationKind::ALL {
            assert!(!kind.label().is_empty());
            assert!(!kind.description().is_empty());
            assert!(kind.duration() > Duration::ZERO);
        }
    }
}
