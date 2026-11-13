//! Handing a search off to lucida.to, so the download lands in the library.
//!
//! Pulse does not download anything, and it never talks to lucida itself.
//! lucida publishes no API and asks not to be scraped, so this module does the
//! one thing that is both allowed and durable: it builds the same URL a person
//! would type, and opens it in the browser. Everything after that — the album
//! page, the format pickers, the download — is lucida's own interface,
//! maintained by them. When their site changes, nothing here breaks.
//!
//! Nothing in this module runs on its own. Every network trip starts with a
//! click, and the files come back through the library's normal folder scan.
//!
//! Two shapes of input are understood:
//!
//! * **A search.** "Portishead Dummy" becomes lucida's search page for the
//!   selected service, which is the same thing as typing it there.
//! * **A link.** Paste an album or track URL from any service lucida reads and
//!   it goes straight to the resolve page, skipping the search.

use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, MouseButton, ParentElement, SharedString, Styled, Subscription,
    Window, actions, div, px, svg,
};
use gpui_component::{
    Disableable, Selectable, Sizable,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::settings_store::{SettingsStore, notify_save_error};
use crate::theme_colors::Colors;

actions!(acquire, [OpenAcquire, CloseAcquire]);

pub const KEY_CONTEXT: &str = "AcquirePanel";

/// The service lucida should pull from, as its search page spells it.
///
/// Only `amazon` has been checked by hand against a real page. The others are
/// lucida's own names as they appear in its interface; a wrong one shows
/// lucida's empty-results page rather than doing anything unexpected, and the
/// setting is a plain string, so it can be corrected without a rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Amazon,
    Qobuz,
    Tidal,
    Deezer,
    AppleMusic,
    SoundCloud,
}

impl Service {
    pub const ALL: [Self; 6] = [
        Self::Amazon,
        Self::Qobuz,
        Self::Tidal,
        Self::Deezer,
        Self::AppleMusic,
        Self::SoundCloud,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Amazon => "amazon",
            Self::Qobuz => "qobuz",
            Self::Tidal => "tidal",
            Self::Deezer => "deezer",
            Self::AppleMusic => "apple",
            Self::SoundCloud => "soundcloud",
        }
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::Amazon => "Amazon Music",
            Self::Qobuz => "Qobuz",
            Self::Tidal => "Tidal",
            Self::Deezer => "Deezer",
            Self::AppleMusic => "Apple Music",
            Self::SoundCloud => "SoundCloud",
        }
        .into()
    }

    /// Unknown slugs fall back to Amazon rather than erroring: the setting is a
    /// string so it can be edited, and a typo should not leave the panel with
    /// no service at all.
    pub fn from_slug(slug: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|service| service.slug().eq_ignore_ascii_case(slug.trim()))
            .unwrap_or(Self::Amazon)
    }
}

/// lucida's search page for one service.
pub fn search_url(service: Service, country: &str, query: &str) -> String {
    format!(
        "https://lucida.to/search?service={}&country={}&query={}",
        service.slug(),
        encode(country.trim()),
        encode(query.trim())
    )
}

/// The resolve page for a link the user pasted.
pub fn resolve_url(service: Service, country: &str, link: &str) -> String {
    format!(
        "https://lucida.to/?url={}&country={}&to={}",
        encode(link.trim()),
        encode(country.trim()),
        service.slug()
    )
}

/// Amazon's own storefront search, for anything lucida cannot resolve.
pub fn amazon_search_url(query: &str) -> String {
    format!(
        "https://music.amazon.com/search/?keywords={}",
        encode(query.trim())
    )
}

/// Whether the text is a link rather than a search.
pub fn is_link(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("https://") || text.starts_with("http://")
}

/// Percent-encode everything that is not unreserved, which is what a query
/// value needs. Hand-rolled because the alternative is a dependency for one
/// function, and because album titles carry `&`, `+` and `#` often enough that
/// getting this wrong is visible immediately.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                out.push('%');
                out.push(hex(byte >> 4));
                out.push(hex(byte & 0x0F));
            }
        }
    }
    out
}

fn hex(nibble: u8) -> char {
    (if nibble < 10 {
        b'0' + nibble
    } else {
        b'A' + nibble - 10
    }) as char
}

/// Hand the URL to the desktop. `xdg-open` is what the rest of the app uses for
/// the same job; a machine without it gets a clipboard copy instead of silence.
fn open_in_browser(url: &str) -> bool {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .is_ok()
}

/// Register the panel's keys. `Ctrl+Shift+G` sits in the main window context so
/// it works from anywhere that is not a text field.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseAcquire, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-g",
            OpenAcquire,
            Some(crate::keyboard_shortcuts::BINDING),
        ),
    ]);
}

#[derive(Clone, Debug)]
pub enum AcquireEvent {
    /// The panel closed, however it closed. The window owner uses this to take
    /// focus back, exactly as the command palette does.
    Closed,
}

pub struct AcquirePanel {
    open: bool,
    input: Entity<InputState>,
    country_input: Entity<InputState>,
    focus_handle: FocusHandle,
    /// Last outcome, shown under the buttons. Cleared on the next search so a
    /// stale "opened in your browser" cannot be mistaken for the new one.
    status: Option<SharedString>,
    _input_subscription: Subscription,
    _country_subscription: Subscription,
}

impl EventEmitter<AcquireEvent> for AcquirePanel {}

impl AcquirePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Album, artist, track — or paste a link")
        });
        let input_subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.hand_off(cx);
            }
        });

        let country = cx.global::<SettingsStore>().acquire_country();
        let country_input = cx.new(|cx| InputState::new(window, cx).default_value(country));
        let country_subscription =
            cx.subscribe(&country_input, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.save_country(cx);
                }
            });

        Self {
            open: false,
            input,
            country_input,
            focus_handle: cx.focus_handle(),
            status: None,
            _input_subscription: input_subscription,
            _country_subscription: country_subscription,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.status = None;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.status = None;
        cx.emit(AcquireEvent::Closed);
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else {
            self.open(window, cx);
        }
    }

    /// The country field writes through on every keystroke, but an empty field
    /// is a half-typed one, not a decision — keep the last good value.
    fn save_country(&mut self, cx: &mut Context<Self>) {
        let value = self.country_input.read(cx).value().trim().to_uppercase();
        if value.is_empty() {
            return;
        }
        if value == cx.global::<SettingsStore>().acquire_country() {
            return;
        }
        if let Err(err) = cx
            .global_mut::<SettingsStore>()
            .set_acquire_country(&value)
        {
            notify_save_error(cx, err);
        }
    }

    /// Build the URL for whatever is in the box and open it. A search and a
    /// pasted link are different pages, so the shape of the input decides.
    fn hand_off(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let settings = cx.global::<SettingsStore>();
        let service = Service::from_slug(&settings.acquire_service());
        let country = settings.acquire_country();
        let url = if is_link(&text) {
            resolve_url(service, &country, &text)
        } else {
            search_url(service, &country, &text)
        };

        self.status = Some(if open_in_browser(&url) {
            "Opened in your browser.".into()
        } else {
            // No `xdg-open` on this machine. The link is still worth having.
            cx.write_to_clipboard(ClipboardItem::new_string(url));
            "No browser found — the link is on your clipboard.".into()
        });
        cx.notify();
    }

    fn choose_service(&mut self, service: Service, cx: &mut Context<Self>) {
        if let Err(err) = cx
            .global_mut::<SettingsStore>()
            .set_acquire_service(service.slug())
        {
            notify_save_error(cx, err);
        }
        cx.notify();
    }

    fn current_service(&self, cx: &App) -> Service {
        Service::from_slug(&cx.global::<SettingsStore>().acquire_service())
    }
}

impl gpui::Render for AcquirePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }

        let popover_bg = crate::cover_backdrop::popover_bg(
            Colors::popover(cx),
            crate::cover_backdrop::veil_factor(cx),
        );
        let border = Colors::border(cx);
        let muted_fg = Colors::muted_foreground(cx);
        let foreground = Colors::foreground(cx);
        let viewport = window.viewport_size();
        let current = self.current_service(cx);
        let query = self.input.read(cx).value().trim().to_string();
        let is_link_now = is_link(&query);

        let panel = cx.entity();
        let backdrop = {
            let panel = panel.clone();
            div()
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .w(viewport.width)
                .h(viewport.height)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    panel.update(cx, |state, cx| state.close(cx));
                })
        };

        let services = h_flex().gap_1().flex_wrap().children(
            Service::ALL
                .into_iter()
                .map(|service| {
                    Button::new(SharedString::from(format!("acquire-service-{}", service.slug())))
                        .small()
                        .label(service.label())
                        .selected(service == current)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_service(service, cx);
                        }))
                })
                .collect::<Vec<_>>(),
        );

        let primary_label = if is_link_now {
            "Open in lucida"
        } else {
            "Search on lucida"
        };

        let status = self.status.clone();

        let card = v_flex()
            .id("acquire-panel")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &CloseAcquire, _, cx| this.close(cx)))
            .w(px(520.))
            // Without this the backdrop is still in the hit-test list for
            // clicks that land on the card, so every press closed the panel
            // before the button under it could see the mouse-up.
            .occlude()
            .overflow_hidden()
            .rounded(px(12.))
            .border_1()
            .border_color(border)
            .bg(popover_bg)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        svg()
                            .path("icons/download.svg")
                            .size(px(16.))
                            .text_color(muted_fg),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.input).focus_bordered(false)),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .px_3()
                    .py_3()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child("Service"),
                            )
                            .child(services),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_xs().text_color(muted_fg).child("Country"))
                            .child(
                                div()
                                    .w(px(72.))
                                    .child(Input::new(&self.country_input).focus_bordered(false)),
                            )
                            .child(
                                div().flex_1().text_xs().text_color(muted_fg).child(
                                    "The storefront lucida resolves against. US, GB, DE…",
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("acquire-primary")
                                    .small()
                                    .label(primary_label)
                                    .on_click(cx.listener(|this, _, _, cx| this.hand_off(cx))),
                            )
                            .child(
                                Button::new("acquire-amazon")
                                    .small()
                                    .label("Amazon Music search")
                                    .disabled(query.is_empty())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let query = this.input.read(cx).value().trim().to_string();
                                        if query.is_empty() {
                                            return;
                                        }
                                        this.status = Some(
                                            if open_in_browser(&amazon_search_url(&query)) {
                                                "Opened in your browser.".into()
                                            } else {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    amazon_search_url(&query),
                                                ));
                                                "No browser found — the link is on your clipboard."
                                                    .into()
                                            },
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .children(status.map(|status| {
                                div().text_xs().text_color(foreground).child(status)
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(
                                "Downloads go to the folder you added in Settings → Online. \
                                 Anything that lands there is indexed on its own.",
                            ),
                    ),
            );

        let top_offset = (viewport.height * 0.16).max(px(48.));

        div()
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .w(viewport.width)
            .h(viewport.height)
            .child(backdrop)
            .child(
                v_flex()
                    .absolute()
                    .top(top_offset)
                    .left(px(0.))
                    .w(viewport.width)
                    .items_center()
                    .child(card),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_is_percent_encoded() {
        assert_eq!(
            search_url(Service::Amazon, "US", "Portishead Dummy"),
            "https://lucida.to/search?service=amazon&country=US&query=Portishead%20Dummy"
        );
    }

    /// `&` and `+` in a title must not turn into a query separator or a space.
    #[test]
    fn punctuation_survives_the_query() {
        let url = search_url(Service::Qobuz, "GB", "Simon & Garfunkel");
        assert!(url.contains("query=Simon%20%26%20Garfunkel"), "{url}");
        assert_eq!(url.matches('&').count(), 2, "only the two separators");
    }

    #[test]
    fn a_pasted_link_goes_to_the_resolve_page() {
        assert_eq!(
            resolve_url(Service::Amazon, "US", "https://music.amazon.com/albums/B01"),
            "https://lucida.to/?url=https%3A%2F%2Fmusic.amazon.com%2Falbums%2FB01&country=US&to=amazon"
        );
    }

    #[test]
    fn a_link_is_told_apart_from_a_search() {
        assert!(is_link("https://music.amazon.com/albums/B01"));
        assert!(is_link("  http://open.qobuz.com/album/1  "));
        assert!(!is_link("Portishead Dummy"));
        assert!(!is_link("music.amazon.com/albums/B01"));
    }

    #[test]
    fn an_unknown_slug_falls_back_to_amazon() {
        assert_eq!(Service::from_slug("nonsense"), Service::Amazon);
        assert_eq!(Service::from_slug(""), Service::Amazon);
        assert_eq!(Service::from_slug(" Qobuz "), Service::Qobuz);
    }

    #[test]
    fn the_country_is_trimmed_and_encoded() {
        assert_eq!(
            search_url(Service::Amazon, " us ", "x"),
            "https://lucida.to/search?service=amazon&country=us&query=x"
        );
    }
}
