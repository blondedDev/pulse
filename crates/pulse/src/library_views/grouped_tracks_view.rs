use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use audio_engine::EngineEvent;
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, App, ClickEvent, Context, Div, ElementId, EventEmitter, FontWeight, Hsla, Image,
    ImageFormat,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point, Render,
    SharedString, Size, Stateful, StatefulInteractiveElement, Styled, Subscription, Window,
    anchored, deferred, div, point, px, size, svg,
};
use gpui_component::{
    VirtualListScrollHandle,
    h_flex,
    scroll::{ScrollableElement, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};


use crate::theme_colors::Colors;
use crate::track_list::{
    ROW_HOVER_GROUP, RowButtonColors, TrackRowBase, add_to_playlist_button, add_to_queue_button,
    append_album_to_queue, append_tracks_to_queue, fmt_track_num, play_next_button,
    track_duration,
};
use nucleo_matcher::{Config, Matcher};
use ui_components::cover_thumb::{cover_fade_id, cover_thumb_fading};

use crate::library_service::LibraryService;
use crate::library_views::fuzzy::fuzzy_scored;
use crate::localization::{LangChanged, tr};
use crate::now_playing::{NavigateToAlbumRequested, NavigateToArtistRequested};
use crate::services::Services;
use crate::settings_store::SettingsStore;
use music_library::ArtistGrouping;

const TRACK_ROW_HEIGHT: f32 = 36.;
const ALBUM_COVER_SIZE: f32 = 60.;
const QUEUE_BTN_SIZE: f32 = 34.;
const QUEUE_ICON_SIZE: f32 = 20.;
const ALBUM_MENU_WIDTH: f32 = 240.;
const PAGE_HEADER_HEIGHT: f32 = 48.;
const ALBUM_HEADER_HEIGHT: f32 = 84.;
const DISC_HEADER_HEIGHT: f32 = 32.;
const DISC_HEADER_GAP: f32 = 24.;

#[derive(Clone, Debug)]
struct TrackRow {
    base: TrackRowBase,
    track_num_str: SharedString,
    disc_number: i32,
}

impl TrackRow {
    fn from_track(track: &music_library::Track) -> Self {
        Self {
            base: TrackRowBase::from_track(track),
            track_num_str: fmt_track_num(track.track_number),
            disc_number: track.disc_number,
        }
    }
}

#[derive(Clone, Debug)]
struct AlbumGroup {
    album_id: Option<i64>,
    album_title: SharedString,
    artist: Option<(i64, SharedString)>,
    year_label: Option<SharedString>,
    /// The album cover as the row's first track carries it. Loaded lazily — see
    /// `prefetch_group_covers`.
    cover_art_id: Option<i64>,
    cover: Option<Arc<Image>>,
    tracks: Vec<TrackRow>,
    /// Indices of `tracks` in the flat artist-wide list (used as playback queue index).
    global_indices: Vec<usize>,
}

#[derive(Clone)]
struct AlbumMenu {
    album_id: i64,
    anchor: Point<Pixels>,
}

struct AlbumMeta {
    title: SharedString,
    artist: Option<(i64, SharedString)>,
}

#[derive(Clone)]
enum ItemKind {
    PageHeader,
    AlbumHeader(usize),
    DiscHeader(SharedString, bool),
    Track(usize, usize),
}

enum Scope {
    Artist { id: i64, grouping: ArtistGrouping },
    /// Every track at or below the folder.
    Folder { path: String },
    /// Only the tracks sitting in the folder itself, not in its
    /// subdirectories. Used by the folder browser's "tracks in this folder"
    /// row, which exists so those files are not hidden by the subdirectory
    /// list.
    FolderDirect { path: String },
}

impl Scope {
    fn fetch(&self, library: &LibraryService) -> Vec<Rc<music_library::Track>> {
        let tracks = match self {
            Scope::Artist { id, grouping } => library.tracks_by_artist(*id, *grouping),
            Scope::Folder { path } => library.tracks_in_folder(path),
            Scope::FolderDirect { path } => library.tracks_directly_in_folder(path),
        };
        tracks.into_iter().map(Rc::new).collect()
    }

    /// An album-shaped listing of a closed, local track set.
    ///
    /// A folder is defined by membership rather than by an artist's
    /// discography: its tracks are grouped as whole albums, an album is never
    /// "partial" so there is nothing to expand, and it only ever holds local
    /// files, so there is nothing for the remote-media cache to fill. An artist
    /// scope is none of those things.
    fn is_album_listing(&self) -> bool {
        matches!(self, Scope::Folder { .. } | Scope::FolderDirect { .. })
    }
}

pub struct GroupedTracksView {
    scope: Scope,
    title: SharedString,
    tracks_all: Vec<Rc<music_library::Track>>,
    albums: HashMap<i64, AlbumMeta>,
    haystacks: Vec<String>,
    groups: Vec<AlbumGroup>,
    items: Vec<ItemKind>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
    filter: String,
    matcher: Matcher,
    current_track_id: Option<i64>,
    is_playing: bool,
    /// Rows picked with ctrl/shift; empty in the normal case.
    selection: crate::track_list::Selection,
    album_menu: Option<AlbumMenu>,
    /// Cover ids being loaded on the background executor, and ids that failed
    /// to load. Together they stop a redraw from queueing the same cover again.
    covers_in_flight: HashSet<i64>,
    covers_unavailable: HashSet<i64>,
    partial_albums: HashSet<i64>,
    show_full_albums: bool,
    _engine_subscription: Subscription,
    _status_subscription: Subscription,
    _lang_subscription: Subscription,
}

impl GroupedTracksView {
    pub fn artist(
        artist: &music_library::ArtistSummary,
        grouping: ArtistGrouping,
        cx: &mut Context<Self>,
    ) -> Self {
        let scope = Scope::Artist {
            id: artist.id,
            grouping,
        };
        Self::new(scope, artist.name.clone().into(), cx)
    }

    pub fn folder(folder: &music_library::FolderSummary, cx: &mut Context<Self>) -> Self {
        let scope = Scope::Folder {
            path: folder.path.clone(),
        };
        Self::new(scope, folder.name.clone().into(), cx)
    }

    /// The folder's own files only, for the "tracks in this folder" row.
    pub fn folder_direct(folder: &music_library::FolderSummary, cx: &mut Context<Self>) -> Self {
        let scope = Scope::FolderDirect {
            path: folder.path.clone(),
        };
        Self::new(scope, folder.name.clone().into(), cx)
    }

    fn new(scope: Scope, title: SharedString, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<Services>();
        let engine_event_bus = services.engine_event_bus.clone();
        let lang_event_bus = services.lang_event_bus.clone();
        let tracks_all = scope.fetch(&services.library);
        let albums = album_meta(&services.library);
        let haystacks = build_haystacks(&scope, &tracks_all, &albums);

        let groups = group_runs(
            &tracks_all,
            0..tracks_all.len(),
            &albums,
            scope.is_album_listing(),
        );
        let partial_albums = if scope.is_album_listing() {
            HashSet::new()
        } else {
            Self::compute_partial_albums(&tracks_all, &services.library)
        };
        let (items, sizes) = Self::build_items(&groups, tr());

        let current_track_id = services
            .playback_queue
            .borrow()
            .current_track()
            .map(|t| t.id);
        let is_playing = services
            .is_playing
            .load(std::sync::atomic::Ordering::Relaxed);

        let engine_subscription = cx.subscribe(
            &engine_event_bus,
            |this, _, event: &EngineEvent, cx| match event {
                EngineEvent::Playing if !this.is_playing => {
                    this.is_playing = true;
                    cx.notify();
                }
                EngineEvent::Paused if this.is_playing => {
                    this.is_playing = false;
                    cx.notify();
                }
                EngineEvent::TrackEnded if this.is_playing => {
                    this.is_playing = false;
                    cx.notify();
                }
                _ => {}
            },
        );

        let playback_status = cx.global::<Services>().playback_status.clone();
        let status_subscription = cx.subscribe(
            &playback_status,
            |this, status, _: &crate::playback_status::StatusChanged, cx| {
                let id = status.read(cx).track_id();
                if this.current_track_id != id {
                    this.current_track_id = id;
                    cx.notify();
                }
            },
        );

        let lang_subscription = cx.subscribe(&lang_event_bus, |this, _, _: &LangChanged, cx| {
            let (items, sizes) = Self::build_items(&this.groups, tr());
            this.items = items;
            this.item_sizes = Rc::new(sizes);
            cx.notify();
        });

        let scroll_handle = VirtualListScrollHandle::new();
        if let Some(track_id) = current_track_id
            && let Some(item_ix) = items.iter().position(|item| {
                matches!(item, ItemKind::Track(g_ix, t_ix)
                    if groups[*g_ix].tracks[*t_ix].base.id == track_id)
            })
        {
            scroll_handle.scroll_to_item(item_ix, gpui::ScrollStrategy::Center);
        }

        let mut this = Self {
            scope,
            title,
            tracks_all,
            albums,
            haystacks,
            groups,
            items,
            item_sizes: Rc::new(sizes),
            scroll_handle,
            filter: String::new(),
            matcher: Matcher::new(Config::DEFAULT),
            current_track_id,
            is_playing,
            selection: crate::track_list::Selection::default(),
            album_menu: None,
            covers_in_flight: HashSet::new(),
            covers_unavailable: HashSet::new(),
            partial_albums,
            show_full_albums: false,
            _engine_subscription: engine_subscription,
            _status_subscription: status_subscription,
            _lang_subscription: lang_subscription,
        };
        // `new` builds the groups directly, so the first batch of covers has to
        // be queued here; every later rebuild goes through `recompute_groups`.
        this.prefetch_group_covers(cx);
        this
    }

    /// Fill the album headers' covers without touching the database on the UI
    /// thread.
    ///
    /// A folder like a 1,400-track "liked" dump groups into ~900 albums, and
    /// reading each small blob synchronously while the view is built froze the
    /// window for hundreds of milliseconds. Whatever is already cached is taken
    /// at once; the rest is read on the background executor and dropped in as it
    /// arrives, each header showing its placeholder until then.
    fn prefetch_group_covers(&mut self, cx: &mut Context<Self>) {
        let cache = cx.global::<Services>().cover_art_cache.clone();
        let mut wanted: Vec<i64> = Vec::new();
        {
            let cache = cache.borrow();
            for group in &mut self.groups {
                if group.cover.is_some() {
                    continue;
                }
                let Some(id) = group.cover_art_id else { continue };
                if let Some(image) = cache.peek_small(Some(id)) {
                    group.cover = Some(image);
                } else if !self.covers_in_flight.contains(&id)
                    && !self.covers_unavailable.contains(&id)
                {
                    wanted.push(id);
                }
            }
        }
        if wanted.is_empty() {
            return;
        }
        wanted.sort_unstable();
        wanted.dedup();
        for id in &wanted {
            self.covers_in_flight.insert(*id);
        }

        let access = cx.global::<Services>().library.library_access();
        cx.spawn(async move |this, cx| {
            for id in wanted {
                let access = access.clone();
                let loaded = cx
                    .background_executor()
                    .spawn(async move {
                        access
                            .cover_small(id)
                            .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes)))
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.covers_in_flight.remove(&id);
                    let Some(image) = loaded else {
                        view.covers_unavailable.insert(id);
                        return;
                    };
                    cx.global::<Services>()
                        .cover_art_cache
                        .borrow_mut()
                        .insert_small(id, image.clone());
                    let mut filled = false;
                    for group in &mut view.groups {
                        if group.cover_art_id == Some(id) && group.cover.is_none() {
                            group.cover = Some(image.clone());
                            filled = true;
                        }
                    }
                    if filled {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Route a click through the selection rules. Returns true when the click
    /// only changed the selection, so the caller must not start playback.
    fn note_selection_click(&mut self, index: usize, window: &mut Window) -> bool {
        let ids: Vec<i64> = self.tracks_all.iter().map(|track| track.id).collect();
        self.selection.click(&ids, index, &window.modifiers())
            == crate::track_list::ClickAction::Selection
    }

    /// The selected tracks, in the order the list shows them.
    fn selected_tracks(&self) -> Vec<Rc<music_library::Track>> {
        let ids: Vec<i64> = self.tracks_all.iter().map(|track| track.id).collect();
        let wanted = self.selection.ids_in_order(&ids);
        self.tracks_all
            .iter()
            .filter(|track| wanted.contains(&track.id))
            .cloned()
            .collect()
    }

    fn compute_partial_albums(
        tracks: &[Rc<music_library::Track>],
        library: &crate::library_service::LibraryService,
    ) -> HashSet<i64> {
        let album_totals = library.album_track_counts();
        let mut artist_counts: std::collections::HashMap<i64, i64> =
            std::collections::HashMap::new();
        for track in tracks {
            if let Some(album_id) = track.album_id {
                *artist_counts.entry(album_id).or_default() += 1;
            }
        }
        artist_counts
            .into_iter()
            .filter(|(album_id, mine)| album_totals.get(album_id).copied().unwrap_or(0) > *mine)
            .map(|(album_id, _)| album_id)
            .collect()
    }

    fn build_items(
        groups: &[AlbumGroup],
        strings: &ui_resources::i18n::Strings,
    ) -> (Vec<ItemKind>, Vec<Size<Pixels>>) {
        let mut items = vec![ItemKind::PageHeader];
        let mut sizes = vec![size(px(300.), px(PAGE_HEADER_HEIGHT))];
        for (g_ix, g) in groups.iter().enumerate() {
            items.push(ItemKind::AlbumHeader(g_ix));
            sizes.push(size(px(300.), px(ALBUM_HEADER_HEIGHT + 1.)));
            let max_disc = g.tracks.iter().map(|t| t.disc_number).max().unwrap_or(1);
            let multi_disc = max_disc > 1;
            let mut current_disc = 0i32;
            for (t_ix, track) in g.tracks.iter().enumerate() {
                if multi_disc && track.disc_number != current_disc {
                    let gap = current_disc != 0;
                    current_disc = track.disc_number;
                    items.push(ItemKind::DiscHeader(
                        strings.disc(current_disc as u32).into(),
                        gap,
                    ));
                    let extra = if gap { DISC_HEADER_GAP } else { 0. };
                    sizes.push(size(px(300.), px(DISC_HEADER_HEIGHT + extra + 1.)));
                }
                items.push(ItemKind::Track(g_ix, t_ix));
                sizes.push(size(px(300.), px(TRACK_ROW_HEIGHT + 1.)));
            }
        }
        (items, sizes)
    }

    pub fn set_filter(&mut self, query: &str, cx: &mut Context<Self>) {
        let trimmed = query.trim().to_string();
        if trimmed == self.filter {
            return;
        }
        self.filter = trimmed;
        self.album_menu = None;
        // A shift-range is measured against the full list, so a selection that
        // survives a filter change could silently include hidden rows.
        self.selection.clear();
        self.recompute_groups(cx);
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn recompute_groups(&mut self, cx: &mut Context<Self>) {
        let with_artist = self.scope.is_album_listing();
        self.groups = if self.filter.is_empty() {
            group_runs(
                &self.tracks_all,
                0..self.tracks_all.len(),
                &self.albums,
                with_artist,
            )
        } else {
            let matches = fuzzy_scored(
                &mut self.matcher,
                &self.filter,
                self.tracks_all.iter().enumerate().map(|(ix, t)| {
                    let hay = self.haystacks.get(ix).unwrap_or(&t.title);
                    (ix, hay.as_str())
                }),
            );
            group_runs(
                &self.tracks_all,
                matches.into_iter().map(|(ix, _)| ix),
                &self.albums,
                with_artist,
            )
        };
        let (items, sizes) = Self::build_items(&self.groups, tr());
        self.items = items;
        self.item_sizes = Rc::new(sizes);
        self.prefetch_group_covers(cx);
    }

    fn rebuild_source(&mut self, cx: &mut Context<Self>) {
        let library = cx.global::<Services>().library.clone();
        let artist_tracks = self.scope.fetch(&library);
        if self.show_full_albums && !self.partial_albums.is_empty() {
            let mut combined: Vec<Rc<music_library::Track>> = Vec::new();
            let mut i = 0;
            while i < artist_tracks.len() {
                let album_id = artist_tracks[i].album_id;
                let mut j = i;
                while j < artist_tracks.len() && artist_tracks[j].album_id == album_id {
                    j += 1;
                }
                match album_id {
                    Some(aid) if self.partial_albums.contains(&aid) => {
                        combined.extend(library.tracks_for_album(aid).into_iter().map(Rc::new))
                    }
                    _ => combined.extend_from_slice(&artist_tracks[i..j]),
                }
                i = j;
            }
            self.tracks_all = combined;
        } else {
            self.tracks_all = artist_tracks;
        }
        self.albums = album_meta(&library);
        self.haystacks = build_haystacks(&self.scope, &self.tracks_all, &self.albums);
        self.album_menu = None;
        self.recompute_groups(cx);
    }

    fn header_name(&self) -> SharedString {
        match self.scope {
            Scope::Artist { id, .. } if id == music_library::NO_METADATA_ARTIST_ID => {
                tr().no_metadata.clone()
            }
            _ => self.title.clone(),
        }
    }

    fn empty_message(&self) -> SharedString {
        match self.scope {
            Scope::Artist { .. } => tr().no_tracks_for_artist.clone(),
            Scope::Folder { .. } | Scope::FolderDirect { .. } => {
                tr().no_tracks_in_folder.clone()
            }
        }
    }
}

fn album_meta(library: &LibraryService) -> HashMap<i64, AlbumMeta> {
    library
        .albums()
        .into_iter()
        .map(|album| {
            let artist = album
                .artist_id
                .map(|id| (id, SharedString::from(album.artist_name)));
            let meta = AlbumMeta {
                title: album.title.into(),
                artist,
            };
            (album.id, meta)
        })
        .collect()
}

/// The search haystacks for a track list.
///
/// Empty means "match the title only", which is what every remaining scope does;
/// `recompute_groups` falls back to `Track::title` in that case. It is kept as a
/// function because an album/artist-enriched haystack is a per-view decision and
/// the call sites are shared.
fn build_haystacks(
    scope: &Scope,
    tracks: &[Rc<music_library::Track>],
    albums: &HashMap<i64, AlbumMeta>,
) -> Vec<String> {
    let _ = (tracks, albums);
    match scope {
        Scope::Artist { .. } | Scope::Folder { .. } | Scope::FolderDirect { .. } => Vec::new(),
    }
}

fn group_runs(
    tracks: &[Rc<music_library::Track>],
    indices: impl IntoIterator<Item = usize>,
    albums: &HashMap<i64, AlbumMeta>,
    with_artist: bool,
) -> Vec<AlbumGroup> {
    let mut groups: Vec<AlbumGroup> = Vec::new();
    for ix in indices {
        let track = &tracks[ix];
        let album_id = track.album_id;
        if let Some(last) = groups.last_mut()
            && last.album_id == album_id
        {
            last.tracks.push(TrackRow::from_track(track));
            last.global_indices.push(ix);
            continue;
        }
        let meta = album_id.and_then(|id| albums.get(&id));
        let album_title = meta
            .map(|m| m.title.clone())
            .unwrap_or_else(|| SharedString::new_static("Unknown"));
        let artist = meta.filter(|_| with_artist).and_then(|m| m.artist.clone());
        groups.push(AlbumGroup {
            album_id,
            album_title,
            artist,
            year_label: track.year.map(|y| y.to_string().into()),
            cover_art_id: track.cover_art_id,
            cover: None,
            tracks: vec![TrackRow::from_track(track)],
            global_indices: vec![ix],
        });
    }
    groups
}

impl EventEmitter<NavigateToAlbumRequested> for GroupedTracksView {}
impl EventEmitter<NavigateToArtistRequested> for GroupedTracksView {}

impl Render for GroupedTracksView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = Colors::border(cx);
        let list_hover = Colors::list_hover(cx);
        let muted_fg = Colors::muted_foreground(cx);
        let foreground = Colors::foreground(cx);
        let fallback_bg = Colors::secondary(cx);
        let fallback_fg = Colors::muted_foreground(cx);
        let popover_bg = Colors::popover(cx);
        let viewport = window.viewport_size();
        let playlists_enabled = cx.global::<SettingsStore>().playlists_enabled();
        let tag_editor_enabled = cx.global::<SettingsStore>().tag_editor_enabled();

        if self.tracks_all.is_empty() {
            return v_flex()
                .size_full()
                .child(page_header_static(self.header_name()))
                .child(div().px_4().child(self.empty_message()));
        }

        if self.groups.is_empty() {
            return v_flex()
                .size_full()
                .child(page_header_static(self.header_name()))
                .child(div().px_4().child(tr().no_tracks_match.clone()));
        }

        let p = ArtistTrackRowParams {
            border,
            list_hover,
            muted_fg,
            foreground,
            playlists_enabled,
            tag_editor_enabled,
            buttons: RowButtonColors::from_cx(cx),
        };
        let item_sizes = self.item_sizes.clone();
        let selection_bar = (!self.selection.is_empty()).then(|| {
            crate::track_list::selection_bar(
                self.selected_tracks(),
                cx.listener(|this, _, _, cx| {
                    this.selection.clear();
                    cx.notify();
                }),
                cx,
            )
        });
        v_flex()
            .size_full()
            .relative()
            .children(selection_bar)
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    "grouped_tracks_list",
                    item_sizes,
                    move |view, visible_range, _window, cx| {
                        visible_range
                            .map(|ix| match &view.items[ix] {
                                ItemKind::PageHeader => page_header(view, muted_fg, cx),
                                ItemKind::DiscHeader(disc, gap) => {
                                    artist_disc_header(disc.clone(), *gap, border, muted_fg)
                                }
                                ItemKind::AlbumHeader(g_ix) => artist_album_header(
                                    view,
                                    *g_ix,
                                    border,
                                    fallback_bg,
                                    fallback_fg,
                                    muted_fg,
                                    cx,
                                ),
                                ItemKind::Track(g_ix, t_ix) => {
                                    artist_track_row(view, *g_ix, *t_ix, &p, cx)
                                }
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.scroll_handle)
                .flex_1(),
            )
            .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical)
            .when_some(self.album_menu.clone(), |el, menu| {
                el.child(album_menu_overlay(
                    &menu,
                    viewport,
                    popover_bg,
                    border,
                    foreground,
                    fallback_bg,
                    cx,
                ))
            })
    }
}

fn artist_disc_header(
    disc: SharedString,
    gap: bool,
    border: gpui::Hsla,
    muted_fg: gpui::Hsla,
) -> gpui::AnyElement {
    let extra = if gap { DISC_HEADER_GAP } else { 0. };
    h_flex()
        .w_full()
        .h(px(DISC_HEADER_HEIGHT + extra))
        .px_4()
        .pb_2()
        .items_end()
        .border_b(px(1.))
        .border_color(border)
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .child(disc),
        )
        .into_any_element()
}

fn artist_album_header(
    view: &mut GroupedTracksView,
    g_ix: usize,
    border: gpui::Hsla,
    fallback_bg: gpui::Hsla,
    fallback_fg: gpui::Hsla,
    muted_fg: gpui::Hsla,
    cx: &mut Context<GroupedTracksView>,
) -> gpui::AnyElement {
    let group = &view.groups[g_ix];
    let cover_el = cover_thumb_fading(
        group.cover.as_ref(),
        ALBUM_COVER_SIZE,
        4.,
        fallback_bg,
        fallback_fg,
        cover_fade_id(group.cover_art_id, group.cover.is_some()),
        crate::animations::cover_fade(cx),
    );
    let album_id = group.album_id;
    let label = match album_id {
        None => tr().no_metadata.clone(),
        Some(_) => group.album_title.clone(),
    };
    let year = group.year_label.clone();
    let title_el = match album_id {
        Some(aid) => div()
            .id(("artist_album_link", aid as u64))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .border_b(px(1.))
            .hover(|s| s.border_color(muted_fg))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(NavigateToAlbumRequested { album_id: aid });
            }))
            .child(label)
            .into_any_element(),
        None => div()
            .font_weight(FontWeight::SEMIBOLD)
            .child(label)
            .into_any_element(),
    };
    let artist_el = group.artist.clone().map(|(artist_id, name)| {
        div()
            .id(("grouped_album_artist", g_ix))
            .cursor_pointer()
            .border_b(px(1.))
            .hover(|s| s.border_color(muted_fg))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(NavigateToArtistRequested {
                    artist_id,
                    track_id: None,
                });
            }))
            .child(name)
    });
    let has_artist = artist_el.is_some();
    let has_year = year.is_some();
    let trigger_hover = Colors::muted(cx);
    let menu_album = if view.show_full_albums {
        None
    } else {
        album_id.filter(|aid| view.partial_albums.contains(aid))
    };
    h_flex()
        .w_full()
        .h(px(ALBUM_HEADER_HEIGHT))
        .px_4()
        .gap_3()
        .items_center()
        .border_b(px(1.))
        .border_color(border)
        .child(cover_el)
        .child(
            v_flex()
                .flex_1()
                .overflow_hidden()
                .gap_1()
                .child(h_flex().text_lg().child(title_el))
                .when(has_artist || has_year, |el| {
                    el.child(
                        h_flex()
                            .gap_1()
                            .text_sm()
                            .text_color(muted_fg)
                            .when_some(artist_el, |el, artist| el.child(artist))
                            .when(has_artist && has_year, |el| el.child("·"))
                            .when_some(year, |el, year| el.child(year)),
                    )
                }),
        )
        .child(album_queue_trigger(
            g_ix,
            menu_album,
            muted_fg,
            trigger_hover,
            cx,
        ))
        .into_any_element()
}

fn album_queue_trigger(
    g_ix: usize,
    menu_album: Option<i64>,
    icon_color: Hsla,
    hover_bg: Hsla,
    cx: &mut Context<GroupedTracksView>,
) -> Stateful<Div> {
    let base = div()
        .id(("artist-album-queue", g_ix))
        .size(px(QUEUE_BTN_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(
            svg()
                .path("icons/add-queue.svg")
                .size(px(QUEUE_ICON_SIZE))
                .text_color(icon_color),
        )
        .tooltip(|window, cx| Tooltip::new(tr().add_to_queue.clone()).build(window, cx));
    match menu_album {
        Some(aid) => base.on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
            this.album_menu = Some(AlbumMenu {
                album_id: aid,
                anchor: ev.position(),
            });
            cx.notify();
        })),
        None => base.on_click(cx.listener(move |this, _, _, cx| {
            let tracks = group_tracks(this, g_ix);
            append_tracks_to_queue(tracks, cx);
        })),
    }
}

fn album_menu_overlay(
    menu: &AlbumMenu,
    viewport: Size<Pixels>,
    popover_bg: Hsla,
    border: Hsla,
    foreground: Hsla,
    hover_bg: Hsla,
    cx: &mut Context<GroupedTracksView>,
) -> gpui::AnyElement {
    let album_id = menu.album_id;
    let backdrop = div()
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .w(viewport.width)
        .h(viewport.height)
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.album_menu = None;
                cx.notify();
            }),
        );
    let content = v_flex()
        .min_w(px(ALBUM_MENU_WIDTH))
        .bg(popover_bg)
        .border_1()
        .border_color(border)
        .rounded(px(8.))
        .shadow_md()
        .p_1()
        .occlude()
        .child(
            queue_menu_row(
                "artist-album-queue-artist".into(),
                "icons/s1-artists.svg",
                tr().add_artist_tracks_to_queue.clone(),
                foreground,
                hover_bg,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.album_menu = None;
                let tracks = artist_tracks_for_album(this, album_id);
                append_tracks_to_queue(tracks, cx);
                cx.notify();
            })),
        )
        .child(
            queue_menu_row(
                "artist-album-queue-album".into(),
                "icons/s1-albums.svg",
                tr().add_album_to_queue.clone(),
                foreground,
                hover_bg,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.album_menu = None;
                append_album_to_queue(album_id, cx);
                cx.notify();
            })),
        );
    let anchor = point(menu.anchor.x, menu.anchor.y + px(8.));
    let menu_layer = deferred(
        anchored()
            .anchor(Anchor::TopRight)
            .snap_to_window_with_margin(px(8.))
            .position(anchor)
            .child(div().occlude().child(content)),
    )
    .with_priority(2);
    let backdrop_layer =
        deferred(anchored().position(point(px(0.), px(0.))).child(backdrop)).with_priority(1);
    div()
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .size_full()
        .child(backdrop_layer)
        .child(menu_layer)
        .into_any_element()
}

fn group_tracks(view: &GroupedTracksView, g_ix: usize) -> Vec<Rc<music_library::Track>> {
    let Some(group) = view.groups.get(g_ix) else {
        return Vec::new();
    };
    group
        .global_indices
        .iter()
        .map(|&ix| view.tracks_all[ix].clone())
        .collect()
}

fn artist_tracks_for_album(
    view: &GroupedTracksView,
    album_id: i64,
) -> Vec<Rc<music_library::Track>> {
    view.tracks_all
        .iter()
        .filter(|t| t.album_id == Some(album_id))
        .cloned()
        .collect()
}

fn queue_menu_row(
    id: ElementId,
    icon: &'static str,
    label: SharedString,
    foreground: Hsla,
    hover_bg: Hsla,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .gap_2()
        .px_2()
        .py_1p5()
        .rounded(px(4.))
        .cursor_pointer()
        .text_sm()
        .text_color(foreground)
        .hover(move |s| s.bg(hover_bg))
        .child(
            svg()
                .path(icon)
                .size(px(14.))
                .flex_shrink_0()
                .text_color(foreground),
        )
        .child(div().whitespace_nowrap().child(label))
}

struct ArtistTrackRowParams {
    border: gpui::Hsla,
    list_hover: gpui::Hsla,
    muted_fg: gpui::Hsla,
    foreground: gpui::Hsla,
    playlists_enabled: bool,
    tag_editor_enabled: bool,
    buttons: RowButtonColors,
}

fn artist_track_row(
    view: &mut GroupedTracksView,
    g_ix: usize,
    t_ix: usize,
    p: &ArtistTrackRowParams,
    cx: &mut Context<GroupedTracksView>,
) -> gpui::AnyElement {
    let group = &view.groups[g_ix];
    let track = &group.tracks[t_ix];
    let global_ix = group.global_indices[t_ix];
    let track_id = track.base.id;
    let is_current = Some(track_id) == view.current_track_id;
    let is_playing = view.is_playing;
    let track_for_queue = view.tracks_all[global_ix].clone();
    let track_for_hover = track_for_queue.clone();

    let leading = if is_current {
        let icon = if is_playing {
            "icons/play.svg"
        } else {
            "icons/pause.svg"
        };
        div()
            .w_8()
            .flex()
            .items_center()
            .child(svg().path(icon).size(px(12.)).text_color(p.foreground))
    } else {
        div()
            .w_8()
            .text_color(p.muted_fg)
            .child(track.track_num_str.clone())
    };

    h_flex()
        .group(ROW_HOVER_GROUP)
        .w_full()
        .h(px(TRACK_ROW_HEIGHT))
        .pl_4()
        .pr_2()
        .gap_2()
        .items_center()
        .border_b(px(1.))
        .border_color(p.border)
        .when(is_current, |s| crate::track_list::current_row(s, cx))
        .when(
            !is_current && view.selection.contains(track_id),
            |s| s.bg(crate::track_list::selected_row_bg(cx)),
        )
        .hover(|s| s.bg(p.list_hover))
        .child(leading)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .text_ellipsis()
                .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(track.base.title.clone()),
        )
        .when(p.tag_editor_enabled && track.base.local, |el| {
            el.child(crate::track_list::edit_tags_button(
                track_for_queue.clone(),
                &p.buttons,
            ))
        })
        .when(p.playlists_enabled, |el| {
            el.child(add_to_playlist_button(track_id, &p.buttons))
        })
        .child(track_duration(cx, track.base.duration.clone()))
        .child(play_next_button(track_for_queue.clone(), 26., 16., &p.buttons))
        .child(add_to_queue_button(track_for_queue, 26., 16., &p.buttons))
        // A folder row is one *file*, and one song adopted from two folders is
        // two rows with the same id, so the index has to name the row.
        .id(ElementId::Name(
            format!("track-{track_id}-{global_ix}").into(),
        ))
        .on_hover(move |hovered: &bool, _, cx| {
            crate::track_list::note_hovered(cx, &track_for_hover, *hovered)
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if this.note_selection_click(global_ix, window) {
                return;
            }
            crate::track_list::play_replacing_queue(
                this.tracks_all.clone(),
                global_ix,
                crate::playback_queue::QueueSource::Unknown,
                Some(this.title.clone()),
                window,
                cx,
            );
        }))
        .into_any_element()
}

fn page_header(
    view: &GroupedTracksView,
    muted_fg: Hsla,
    cx: &mut Context<GroupedTracksView>,
) -> gpui::AnyElement {
    let title = div()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .text_ellipsis()
        .text_xl()
        .font_weight(FontWeight::SEMIBOLD)
        .child(view.header_name());
    let row = h_flex()
        .w_full()
        .h(px(PAGE_HEADER_HEIGHT))
        .pl_4()
        .pr_6()
        .gap_3()
        .items_center()
        .child(title);
    match &view.scope {
        Scope::Artist { .. } => artist_header_controls(row, view, muted_fg, cx),
        // A folder has no sort preference of its own; its tracks come back in
        // the library's display order.
        Scope::Folder { .. } | Scope::FolderDirect { .. } => row,
    }
    .into_any_element()
}

fn artist_header_controls(
    row: Div,
    view: &GroupedTracksView,
    muted_fg: Hsla,
    cx: &mut Context<GroupedTracksView>,
) -> Div {
    row.when(!view.partial_albums.is_empty(), |el| {
        let on = view.show_full_albums;
        let primary = Colors::primary(cx);
        let primary_fg = Colors::primary_foreground(cx);
        let accent = Colors::accent(cx);
        el.child(full_albums_icon(
            on, primary, primary_fg, muted_fg, accent, cx,
        ))
    })
}

fn toggle_full_albums(this: &mut GroupedTracksView, cx: &mut Context<GroupedTracksView>) {
    this.show_full_albums = !this.show_full_albums;
    this.rebuild_source(cx);
    cx.notify();
}

fn full_albums_tooltip(window: &mut Window, cx: &mut App) -> gpui::AnyView {
    Tooltip::element(|_, _| div().w(px(260.)).child(tr().full_albums_tooltip.clone()))
        .build(window, cx)
}

fn full_albums_icon(
    on: bool,
    primary: Hsla,
    primary_fg: Hsla,
    muted_fg: Hsla,
    accent: Hsla,
    cx: &mut Context<GroupedTracksView>,
) -> Stateful<Div> {
    let icon_color = if on { primary_fg } else { muted_fg };
    div()
        .id("artist-full-albums-icon")
        .size(px(QUEUE_BTN_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .map(|el| {
            if on {
                el.bg(primary)
            } else {
                el.hover(move |s| s.bg(accent))
            }
        })
        .child(
            svg()
                .path("icons/s1-albums.svg")
                .size(px(QUEUE_ICON_SIZE))
                .text_color(icon_color),
        )
        .on_click(cx.listener(|this, _, _, cx| toggle_full_albums(this, cx)))
        .tooltip(full_albums_tooltip)
}

fn page_header_static(name: SharedString) -> gpui::Div {
    div().px_4().pt_3().pb_2().child(
        div()
            .text_xl()
            .font_weight(FontWeight::SEMIBOLD)
            .child(name),
    )
}
