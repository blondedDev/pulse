use std::time::Duration;

use gpui::{
    App, AppContext, Context, Empty, Entity, Global, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Task, Window, div, px, svg,
};
use gpui_component::tooltip::Tooltip;

use crate::library_service::LibraryEvent;
use crate::localization::tr;
use crate::services::Services;
use crate::theme_colors::Colors;

const DONE_VISIBLE: Duration = Duration::from_secs(30);
const ICON_SIZE: f32 = 14.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Done {
    Updated,
    UpToDate,
}

pub struct LibraryScanIndicator {
    scanning: bool,
    done: Option<Done>,
    _hide: Option<Task<()>>,
    _subscription: Subscription,
}

struct IndicatorGlobal(Entity<LibraryScanIndicator>);

impl Global for IndicatorGlobal {}

pub fn setup(cx: &mut App) {
    let indicator = cx.new(LibraryScanIndicator::new);
    cx.set_global(IndicatorGlobal(indicator));
}

pub fn indicator(cx: &App) -> Entity<LibraryScanIndicator> {
    cx.global::<IndicatorGlobal>().0.clone()
}

impl LibraryScanIndicator {
    fn new(cx: &mut Context<Self>) -> Self {
        let bus = cx.global::<Services>().library_event_bus.clone();
        let subscription = cx.subscribe(&bus, |this, _, event: &LibraryEvent, cx| {
            match event {
                LibraryEvent::ScanStarted => {
                    this.scanning = true;
                    this.clear_done(cx);
                }
                LibraryEvent::ScanSucceeded => {
                    this.scanning = false;
                    this.finish(Done::Updated, cx);
                }
                LibraryEvent::ScanUpToDate => {
                    this.scanning = false;
                    this.finish(Done::UpToDate, cx);
                }
                LibraryEvent::ScanFailed => {
                    this.scanning = false;
                    this.clear_done(cx);
                }
                _ => {}
            };
        });
        Self {
            scanning: false,
            done: None,
            _hide: None,
            _subscription: subscription,
        }
    }

    fn clear_done(&mut self, cx: &mut Context<Self>) {
        self.done = None;
        self._hide = None;
        cx.notify();
    }

    fn finish(&mut self, done: Done, cx: &mut Context<Self>) {
        self.clear_done(cx);
        self.done = Some(done);
        self._hide = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DONE_VISIBLE).await;
            let _ = this.update(cx, |this, cx| this.clear_done(cx));
        }));
    }
}

impl Render for LibraryScanIndicator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (icon, color, label): (&'static str, Hsla, SharedString) = if self.scanning {
            (
                "icons/refresh.svg",
                Colors::foreground(cx),
                tr().library_updating.clone(),
            )
        } else {
            let muted = Colors::muted_foreground(cx);
            match self.done {
                Some(Done::Updated) => ("icons/check.svg", muted, tr().library_updated.clone()),
                Some(Done::UpToDate) => ("icons/check.svg", muted, tr().library_up_to_date.clone()),
                None => return Empty.into_any_element(),
            }
        };
        div()
            .id("library_scan_indicator")
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(svg().path(icon).size(px(ICON_SIZE)).text_color(color))
            .into_any_element()
    }
}
