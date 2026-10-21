//! Pulse Rewind: what the listening history adds up to.
//!
//! Everything here is a query over `plays` — no separate statistics table to
//! drift out of sync, and no aggregation job to run. A period is just a
//! timestamp lower bound, which is why switching between them is instant.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::FluentBuilder;
use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use gpui_component::{h_flex, v_flex};
use music_library::{ListeningStats, StatEntry};

use crate::library_service::LibraryEvent;
use crate::services::Services;
use crate::theme_colors::Colors;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Span {
    All,
    Year,
    Quarter,
}

impl Span {
    const ALL: [Self; 3] = [Self::All, Self::Year, Self::Quarter];

    fn label(self) -> &'static str {
        match self {
            Self::All => "all time",
            Self::Year => "last 12 months",
            Self::Quarter => "last 90 days",
        }
    }

    /// A lower bound on `plays.started_at`.
    ///
    /// Deliberately a rolling window rather than a calendar year: "last 12
    /// months" needs no timezone-aware year arithmetic, and it cannot show an
    /// empty view on 1 January.
    fn since(self) -> Option<u64> {
        let now = unix_now();
        match self {
            Self::All => None,
            Self::Year => Some(now.saturating_sub(365 * 86_400)),
            Self::Quarter => Some(now.saturating_sub(90 * 86_400)),
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub struct RewindView {
    span: Span,
    stats: ListeningStats,
    _subscription: Subscription,
}

impl RewindView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let bus = cx.global::<Services>().library_event_bus.clone();
        let subscription = cx.subscribe(&bus, |this, _, event: &LibraryEvent, cx| {
            // A rescan changes what the tracks are, so the summary is worth
            // recomputing; a single play does not, and re-querying on every
            // track change would be noise.
            if matches!(event, LibraryEvent::CatalogChanged) {
                this.reload(cx);
            }
        });
        let mut view = Self {
            span: Span::All,
            stats: ListeningStats::default(),
            _subscription: subscription,
        };
        view.reload(cx);
        view
    }

    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.stats = cx
            .global::<Services>()
            .library
            .listening_stats(self.span.since(), 10);
        cx.notify();
    }
}

impl Render for RewindView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = Colors::muted_foreground(cx);
        let border = Colors::border(cx);
        let active = Colors::list_active(cx);

        let chips = h_flex()
            .gap_1()
            .children(Span::ALL.into_iter().map(|span| {
                let selected = self.span == span;
                div()
                    .id(span.label())
                    .px_2()
                    .py(px(4.))
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .text_xs()
                    .cursor_pointer()
                    .when(selected, |chip| chip.bg(active))
                    .child(span.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.span == span {
                            return;
                        }
                        this.span = span;
                        this.reload(cx);
                    }))
            }));

        if self.stats.is_empty() {
            return v_flex()
                .size_full()
                .gap_4()
                .p_4()
                .child(chips)
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(div().text_color(muted).child("Nothing played in this period yet."))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Rewind counts your listening history, which builds up as you play music."),
                        ),
                )
                .into_any_element();
        }

        let metrics = h_flex()
            .w_full()
            .gap_2()
            .child(metric(
                self.stats.plays.to_string(),
                "plays",
                muted,
                border,
            ))
            .child(metric(
                format_time(self.stats.seconds),
                "listened",
                muted,
                border,
            ))
            .child(metric(
                self.stats.artists.to_string(),
                "artists",
                muted,
                border,
            ))
            .child(metric(
                self.stats.albums.to_string(),
                "albums",
                muted,
                border,
            ))
            .child(metric(
                self.stats.days.to_string(),
                "days",
                muted,
                border,
            ));

        let columns = h_flex()
            .w_full()
            .items_start()
            .gap_6()
            .child(ranked("Top artists", &self.stats.top_artists, muted))
            .child(ranked("Top albums", &self.stats.top_albums, muted))
            .child(ranked("Top tracks", &self.stats.top_tracks, muted));

        v_flex()
            .id("rewind")
            .size_full()
            .gap_5()
            .p_4()
            .overflow_y_scroll()
            .child(chips)
            .child(metrics)
            .child(columns)
            .into_any_element()
    }
}

fn format_time(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn metric(value: String, label: &'static str, muted: Hsla, border: Hsla) -> impl IntoElement {
    v_flex()
        .flex_1()
        .gap_1()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .child(div().text_xl().child(value))
        .child(div().text_xs().text_color(muted).child(label))
}

fn ranked(title: &'static str, entries: &[StatEntry], muted: Hsla) -> impl IntoElement {
    v_flex()
        .flex_1()
        .gap_2()
        .overflow_hidden()
        .child(div().text_xs().text_color(muted).child(title))
        .children(entries.iter().enumerate().map(|(ix, entry)| {
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w(px(16.))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{}", ix + 1)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .overflow_hidden()
                        .child(div().text_sm().text_ellipsis().child(entry.name.clone()))
                        .children(
                            entry
                                .subtitle
                                .clone()
                                .map(|subtitle| div().text_xs().text_color(muted).text_ellipsis().child(subtitle)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{}", entry.plays)),
                )
        }))
}
