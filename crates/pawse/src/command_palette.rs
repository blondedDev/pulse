//! Ctrl+K command palette.
//!
//! One fuzzy search over everything the library can reach — tracks, albums,
//! artists, genres, playlists — plus a few app actions.
//!
//! Two decisions worth knowing before editing this:
//!
//! * **The candidate list is built when the palette opens, never per
//!   keystroke.** Opening is rare; typing is not. Scoring a keystroke against a
//!   snapshot is cheap, querying the library for one is not.
//! * **The result list is a window, not a scroller.** It draws a fixed number of
//!   rows around the selection, so a keyboard selection can never end up off
//!   screen — which is exactly what happens if you render a long list and move
//!   the highlight past the fold.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, MouseButton, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window, actions, div, px, svg,
};
use gpui_component::{
    input::{Input, InputEvent, InputState},
    h_flex,
    v_flex,
};
use nucleo_matcher::{Config, Matcher};

use crate::library_service::LibraryEvent;
use crate::library_views::fuzzy::fuzzy_scored;
use crate::services::Services;
use crate::theme_colors::Colors;

/// Rows drawn at once. Also the cap on how far the window can show.
const VISIBLE_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 36.;
const PANEL_WIDTH: f32 = 620.;

actions!(command_palette, [CloseCommandPalette, PaletteNext, PalettePrev, ToggleCommandPalette]);

pub const KEY_CONTEXT: &str = "CommandPalette";

/// Register the palette's own keys. Scoped to its key context, so they only fire
/// while it is on screen and focused. `Input` propagates keys it does not use,
/// which is why the arrows can be claimed here.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseCommandPalette, Some(KEY_CONTEXT)),
        KeyBinding::new("down", PaletteNext, Some(KEY_CONTEXT)),
        KeyBinding::new("up", PalettePrev, Some(KEY_CONTEXT)),
        // While the palette is up its own input has focus, which is exactly the
        // case the main-window binding below excludes itself from — so the
        // toggle is bound here too, or Ctrl+K would open it and then not close it.
        KeyBinding::new("ctrl-k", ToggleCommandPalette, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "ctrl-k",
            ToggleCommandPalette,
            Some(crate::keyboard_shortcuts::BINDING),
        ),
    ]);
}

/// An app action the palette can run. Kept as a closed enum rather than a
/// closure so it stays `Copy` and the palette does not have to know how any of
/// it works — `MainView` does that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    ToggleQueue,
    ToggleLyrics,
    ToggleCoverMode,
    OpenSettings,
}

impl PaletteAction {
    const ALL: [Self; 4] = [
        Self::ToggleQueue,
        Self::ToggleLyrics,
        Self::ToggleCoverMode,
        Self::OpenSettings,
    ];

    fn label(self) -> SharedString {
        match self {
            Self::ToggleQueue => "Toggle queue",
            Self::ToggleLyrics => "Toggle lyrics",
            Self::ToggleCoverMode => "Toggle cover mode",
            Self::OpenSettings => "Open settings",
        }
        .into()
    }
}

/// What the palette asks the app to do. `MainView` handles these, so the palette
/// never reaches into the library view or the queue itself.
#[derive(Clone, Debug)]
pub enum CommandPaletteEvent {
    /// The palette closed, however it closed (Escape, a chosen result, a
    /// backdrop click). The window owner uses this to take focus back: while the
    /// palette is up its input owns focus, and leaving it there kills every
    /// global shortcut until the window is clicked again.
    Closed,
    PlayTrack {
        track_id: i64,
        album_id: Option<i64>,
    },
    OpenAlbum(i64),
    OpenArtist {
        artist_id: i64,
        from_track: Option<i64>,
    },
    OpenGenre(String),
    OpenPlaylist(i64),
    RunAction(PaletteAction),
}

/// Result groups, in the order they sort and the order they are offered as
/// scopes. Derived `Ord` is the declaration order, which is load-bearing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Action,
    Track,
    Album,
    Artist,
    Genre,
    Playlist,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Self::Action => "Action",
            Self::Track => "Track",
            Self::Album => "Album",
            Self::Artist => "Artist",
            Self::Genre => "Genre",
            Self::Playlist => "Playlist",
        }
    }
}

#[derive(Clone, Debug)]
enum Target {
    Track {
        id: i64,
        album_id: Option<i64>,
    },
    Album(i64),
    Artist {
        id: i64,
    },
    Genre(String),
    Playlist(i64),
    Action(PaletteAction),
}

struct Candidate {
    group: Group,
    label: SharedString,
    detail: SharedString,
    /// What fuzzy matching runs against — label plus whatever else is worth
    /// typing ("artist album" finds a track).
    haystack: String,
    target: Target,
}

/// Read the scope prefix off a query. `>` actions, `@` artists, `a:` albums,
/// `#` genres, `p:` playlists; anything else searches everything.
fn scope_of(query: &str) -> (Option<Group>, &str) {
    for (prefix, group) in [
        (">", Group::Action),
        ("@", Group::Artist),
        ("a:", Group::Album),
        ("#", Group::Genre),
        ("p:", Group::Playlist),
    ] {
        if let Some(rest) = query.strip_prefix(prefix) {
            return (Some(group), rest);
        }
    }
    (None, query)
}

pub struct CommandPalette {
    open: bool,
    query: String,
    input: Entity<InputState>,
    focus_handle: FocusHandle,
    /// Reused across keystrokes; building one is not free.
    matcher: Matcher,
    candidates: Vec<Candidate>,
    /// Indices into `candidates`, already ordered for display.
    results: Vec<usize>,
    selected: usize,
    /// First row drawn, so the selection stays on screen.
    window_start: usize,
    _input_subscription: Subscription,
    _library_subscription: Subscription,
}

impl EventEmitter<CommandPaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(SEARCH_PLACEHOLDER));

        let input_subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            match event {
                InputEvent::Change => {
                    this.query = this.input.read(cx).value().to_string();
                    this.recompute();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.confirm(cx),
                _ => {}
            }
        });

        // The library moving under an open palette would leave stale targets.
        let library_event_bus = cx.global::<Services>().library_event_bus.clone();
        let library_subscription =
            cx.subscribe(&library_event_bus, |this, _, event: &LibraryEvent, cx| {
                if !this.open {
                    return;
                }
                if matches!(event, LibraryEvent::CatalogChanged) {
                    this.candidates = build_candidates(cx);
                    this.recompute();
                    cx.notify();
                }
            });

        Self {
            open: false,
            query: String::new(),
            input,
            focus_handle: cx.focus_handle(),
            matcher: Matcher::new(Config::DEFAULT),
            candidates: Vec::new(),
            results: Vec::new(),
            selected: 0,
            window_start: 0,
            _input_subscription: input_subscription,
            _library_subscription: library_subscription,
        }
    }


    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.query.clear();
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.candidates = build_candidates(cx);
        self.recompute();
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.query.clear();
        self.results.clear();
        cx.emit(CommandPaletteEvent::Closed);
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else {
            self.open(window, cx);
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.results.len();
        self.follow_selection();
        cx.notify();
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.results.len() - 1
        } else {
            self.selected - 1
        };
        self.follow_selection();
        cx.notify();
    }

    /// Keep the drawn window on the selection. Cheap because the window is a
    /// slice, not a scroll offset to negotiate with a scroller.
    fn follow_selection(&mut self) {
        let last_start = self.results.len().saturating_sub(VISIBLE_ROWS);
        if self.selected < self.window_start {
            self.window_start = self.selected;
        } else if self.selected >= self.window_start + VISIBLE_ROWS {
            self.window_start = self.selected + 1 - VISIBLE_ROWS;
        }
        self.window_start = self.window_start.min(last_start);
    }

    fn recompute(&mut self) {
        self.selected = 0;
        self.window_start = 0;
        self.results.clear();

        let (scope, needle) = scope_of(self.query.trim());

        // An empty query is not a search: it offers the actions, which are the
        // only entries that make sense without anything typed.
        if scope.is_none() && needle.trim().is_empty() {
            self.results = self
                .candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| c.group == Group::Action)
                .map(|(ix, _)| ix)
                .collect();
            return;
        }

        if needle.trim().is_empty() {
            // A bare scope prefix ("@"): list that group in its natural order.
            self.results = self
                .candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| Some(c.group) == scope)
                .map(|(ix, _)| ix)
                .collect();
            return;
        }

        let candidates: Vec<(usize, &str)> = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| scope.is_none_or(|scope| c.group == scope))
            .map(|(ix, c)| (ix, c.haystack.as_str()))
            .collect();

        let mut scored = fuzzy_scored(&mut self.matcher, needle, candidates);
        // Group first so the list reads in sections, score within a group so the
        // best match in each section is on top.
        scored.sort_by(|(a_ix, a_score), (b_ix, b_score)| {
            let a_group = self.candidates[*a_ix].group;
            let b_group = self.candidates[*b_ix].group;
            a_group
                .cmp(&b_group)
                .then(b_score.cmp(a_score))
                .then(a_ix.cmp(b_ix))
        });
        self.results = scored.into_iter().map(|(ix, _)| ix).collect();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(&ix) = self.results.get(self.selected) else {
            return;
        };
        let event = match &self.candidates[ix].target {
            Target::Track { id, album_id } => CommandPaletteEvent::PlayTrack {
                track_id: *id,
                album_id: *album_id,
            },
            Target::Album(id) => CommandPaletteEvent::OpenAlbum(*id),
            Target::Artist { id } => CommandPaletteEvent::OpenArtist {
                artist_id: *id,
                from_track: None,
            },
            Target::Genre(name) => CommandPaletteEvent::OpenGenre(name.clone()),
            Target::Playlist(id) => CommandPaletteEvent::OpenPlaylist(*id),
            Target::Action(action) => CommandPaletteEvent::RunAction(*action),
        };
        cx.emit(event);
        self.close(cx);
    }
}

const SEARCH_PLACEHOLDER: &str = "Search tracks, albums, artists…  ( > actions · @ artists · a: albums · # genres · p: playlists )";

/// Snapshot everything searchable. One pass over the library listings, which is
/// a single query rather than one per track.
fn build_candidates(cx: &App) -> Vec<Candidate> {
    let services = cx.global::<Services>();
    let library = &services.library;
    let mut out = Vec::new();

    for action in PaletteAction::ALL {
        let label = action.label();
        out.push(Candidate {
            group: Group::Action,
            label: label.clone(),
            detail: SharedString::default(),
            haystack: label.to_string(),
            target: Target::Action(action),
        });
    }

    for listing in services.library.repo().track_listings().unwrap_or_default() {
        let mut haystack = listing.title.clone();
        if !listing.artist.is_empty() {
            haystack.push(' ');
            haystack.push_str(&listing.artist);
        }
        if let Some(album) = &listing.album {
            haystack.push(' ');
            haystack.push_str(album);
        }
        out.push(Candidate {
            group: Group::Track,
            label: listing.title.clone().into(),
            detail: listing.artist.clone().into(),
            haystack,
            target: Target::Track {
                id: listing.track_id,
                album_id: listing.album_id,
            },
        });
    }

    for album in library.albums() {
        let detail = if album.artist_name.is_empty() {
            album.year.map(|y| y.to_string()).unwrap_or_default()
        } else {
            album.artist_name.clone()
        };
        out.push(Candidate {
            group: Group::Album,
            label: album.title.clone().into(),
            detail: detail.into(),
            haystack: format!("{} {}", album.title, album.artist_name),
            target: Target::Album(album.id),
        });
    }

    let grouping = cx
        .try_global::<crate::settings_store::SettingsStore>()
        .map(|store| store.artists_grouping())
        .unwrap_or_default();
    for artist in library.artists(grouping) {
        out.push(Candidate {
            group: Group::Artist,
            label: artist.name.clone().into(),
            detail: SharedString::default(),
            haystack: artist.name.clone(),
            target: Target::Artist { id: artist.id },
        });
    }

    for genre in library.genres() {
        out.push(Candidate {
            group: Group::Genre,
            label: genre.name.clone().into(),
            detail: SharedString::default(),
            haystack: genre.name.clone(),
            target: Target::Genre(genre.name),
        });
    }

    for playlist in library.playlists() {
        out.push(Candidate {
            group: Group::Playlist,
            label: playlist.name.clone().into(),
            detail: SharedString::default(),
            haystack: playlist.name.clone(),
            target: Target::Playlist(playlist.id),
        });
    }

    out
}

impl gpui::Render for CommandPalette {
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
        let accent_fg = Colors::accent_foreground(cx);
        let hover_bg = Colors::list_hover(cx);
        let selected_bg = Colors::list_active(cx);
        let selected_border = Colors::list_active_border(cx);

        let viewport = window.viewport_size();
        let entity = cx.entity();

        let backdrop = {
            let palette = entity.clone();
            div()
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .w(viewport.width)
                .h(viewport.height)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    palette.update(cx, |state, cx| state.close(cx));
                })
        };

        let window_start = self.window_start.min(self.results.len());
        let window_end = (window_start + VISIBLE_ROWS).min(self.results.len());

        let rows: Vec<AnyElement> = (window_start..window_end)
            .map(|ix| {
                let candidate_ix = self.results[ix];
                let candidate = &self.candidates[candidate_ix];
                let selected = ix == self.selected;
                let group = candidate.group;
                let label = candidate.label.clone();
                let detail = candidate.detail.clone();
                let palette = entity.clone();

                h_flex()
                    .id(("palette-row", ix))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_3()
                    .gap_3()
                    .items_center()
                    .rounded(px(6.))
                    .when(selected, |row| {
                        row.bg(selected_bg).border_1().border_color(selected_border)
                    })
                    .when(!selected, |row| row.hover(move |s| s.bg(hover_bg)))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        palette.update(cx, |state, cx| {
                            state.selected = ix;
                            state.confirm(cx);
                        });
                    })
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(label),
                            ),
                    )
                    .when(!detail.is_empty(), |row| {
                        row.child(
                            div()
                                .max_w(px(220.))
                                .text_xs()
                                .text_color(muted_fg)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(detail),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(if selected { accent_fg } else { muted_fg })
                            .child(group.label()),
                    )
                    .into_any_element()
            })
            .collect();

        let body: AnyElement = if self.query.trim().is_empty() {
            v_flex()
                .w_full()
                .children(rows)
                .child(
                    div()
                        .px_3()
                        .pt_2()
                        .text_xs()
                        .text_color(muted_fg)
                        .child("Type to search. Prefixes narrow: > @ a: # p:"),
                )
                .into_any_element()
        } else if self.results.is_empty() {
            v_flex()
                .px_3()
                .py_4()
                .text_sm()
                .text_color(muted_fg)
                .child("No matches")
                .into_any_element()
        } else {
            v_flex().w_full().children(rows).into_any_element()
        };

        let list = v_flex()
            .id("palette-list")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &CloseCommandPalette, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &PaletteNext, _, cx| this.select_next(cx)))
            .on_action(cx.listener(|this, _: &PalettePrev, _, cx| this.select_prev(cx)))
            .w(px(PANEL_WIDTH))
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
                            .path("icons/s1-search.svg")
                            .size(px(16.))
                            .text_color(muted_fg),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.input).focus_bordered(false)),
                    ),
            )
            .child(body);

        let top_offset = (viewport.height * 0.12).max(px(48.));

        div()
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .w(viewport.width)
            .h(viewport.height)
            .child(backdrop)
            .child(
                v_flex().absolute().top(top_offset).left(px(0.)).w(viewport.width).items_center().child(list),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_query_has_no_scope() {
        assert_eq!(scope_of("pneuma"), (None, "pneuma"));
        assert_eq!(scope_of(""), (None, ""));
    }

    #[test]
    fn prefixes_narrow_to_one_group() {
        assert_eq!(scope_of(">queue"), (Some(Group::Action), "queue"));
        assert_eq!(scope_of("@tool"), (Some(Group::Artist), "tool"));
        assert_eq!(scope_of("a:fear"), (Some(Group::Album), "fear"));
        assert_eq!(scope_of("#metal"), (Some(Group::Genre), "metal"));
        assert_eq!(scope_of("p:mix"), (Some(Group::Playlist), "mix"));
    }

    #[test]
    fn a_bare_prefix_is_a_scope_with_an_empty_needle() {
        assert_eq!(scope_of(">"), (Some(Group::Action), ""));
        assert_eq!(scope_of("@"), (Some(Group::Artist), ""));
    }

    /// The whole point of `a:` rather than `a`: a title that merely starts with a
    /// scope letter must still search everything.
    #[test]
    fn a_word_that_only_starts_like_a_prefix_is_not_one() {
        assert_eq!(scope_of("pink floyd"), (None, "pink floyd"));
        assert_eq!(scope_of("atom heart mother"), (None, "atom heart mother"));
        assert_eq!(scope_of("paranoid android"), (None, "paranoid android"));
    }

    #[test]
    fn groups_sort_in_the_order_they_are_offered() {
        assert!(Group::Action < Group::Track);
        assert!(Group::Track < Group::Album);
        assert!(Group::Album < Group::Artist);
        assert!(Group::Artist < Group::Genre);
        assert!(Group::Genre < Group::Playlist);
    }
}
