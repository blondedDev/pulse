use std::cmp::Ordering;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, ElementId, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, Size, StatefulInteractiveElement, Styled, Subscription, Window,
    div, px, size,
};
use gpui_component::{
    Icon, IconName, Sizable, VirtualListScrollHandle, h_flex,
    scroll::{ScrollableElement, ScrollbarAxis},
    v_flex, v_virtual_list,
};
use nucleo_matcher::{Config, Matcher};

use crate::cover_art_cache::{capacity_for_peak_visible, decode_cover_tile};
use crate::library_service::{LibraryAccess, LibraryEvent};
use crate::library_views::albums_view::{OpenLibrarySettings, empty_library, has_music_sources};
use crate::library_views::{albums_grid, folders_grid};
use crate::library_views::fuzzy::fuzzy_sorted;
use crate::localization::{LangChanged, tr};
use crate::services::Services;
use crate::settings_store::{FolderLayout, FolderSort, SettingsStore};
use crate::theme_colors::Colors;

#[derive(Clone, Debug)]
pub struct FolderSelectedEvent {
    pub folder: music_library::FolderSummary,
}

/// Open the tracks of the folder currently being listed, skipping the
/// "has subdirectories, so drill in" decision.
///
/// A folder can hold subdirectories *and* its own files. The level view lists
/// the subdirectories, so without this row those files would be unreachable.
#[derive(Clone, Debug)]
pub struct FolderTracksRequested {
    pub folder: music_library::FolderSummary,
}

pub(super) struct FolderRow {
    pub(super) summary: music_library::FolderSummary,
    pub(super) name: SharedString,
    count_label: SharedString,
}

impl FolderRow {
    fn build(summary: music_library::FolderSummary) -> Self {
        let count_label = tr().n_tracks(summary.track_count).into();
        let name = summary.name.clone().into();
        Self {
            summary,
            name,
            count_label,
        }
    }
}

pub(super) const TOP_PADDING: f32 = 12.;
pub(super) const FOLDER_ROW_HEIGHT: f32 = 44.;

/// Case-insensitive, so `beatles` and `Beatles` sort together instead of
/// splitting the list at every capital.
fn name_order(a: &music_library::FolderSummary, b: &music_library::FolderSummary) -> Ordering {
    a.name.to_lowercase().cmp(&b.name.to_lowercase())
}

fn sort_folders(folders: &mut [music_library::FolderSummary], sort: FolderSort) {
    // Ties fall back to the name: the order the query returned is not something
    // to rely on, and a re-scan should not shuffle equal track counts.
    match sort {
        FolderSort::Name => folders.sort_by(name_order),
        FolderSort::NameDesc => folders.sort_by(|a, b| name_order(b, a)),
        FolderSort::Tracks => folders.sort_by(|a, b| {
            a.track_count
                .cmp(&b.track_count)
                .then_with(|| name_order(a, b))
        }),
        FolderSort::TracksDesc => folders.sort_by(|a, b| {
            b.track_count
                .cmp(&a.track_count)
                .then_with(|| name_order(a, b))
        }),
    }
}

pub struct FoldersView {
    /// `None` lists the configured library roots; `Some(path)` lists the
    /// immediate subdirectories of that folder instead. This is what makes the
    /// tab drill down one level at a time.
    scope: Option<SharedString>,
    pub(super) folders_all: Vec<music_library::FolderSummary>,
    pub(super) rows: Vec<FolderRow>,
    /// The folder being listed, when it has tracks sitting directly in it.
    /// Rendered as one extra row above the subdirectories so those tracks stay
    /// reachable; `None` when there are none, which is the common case.
    pub(super) own_folder: Option<music_library::FolderSummary>,
    filter: String,
    /// How the subdirectories are ordered while the filter is empty.
    sort: FolderSort,
    /// Whether the level is drawn as rows or as tiles.
    layout: FolderLayout,
    matcher: Matcher,
    is_scanning: bool,
    pub(super) item_sizes: Rc<Vec<Size<Pixels>>>,
    pub(super) scroll_handle: VirtualListScrollHandle,
    /// Grid geometry, measured from the real width in `render` because the
    /// column count depends on the panel, not on a setting.
    pub(super) measured_width: Pixels,
    pub(super) columns: usize,
    pub(super) tile_width: f32,
    pub(super) rem_size: f32,
    library_access: LibraryAccess,
    covers_in_flight: HashSet<i64>,
    covers_unavailable: HashSet<i64>,
    visible_span_peak: usize,
    _subscription: Subscription,
    _lang_subscription: Subscription,
    _settings_observer: Subscription,
}

impl FoldersView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::scoped(None, cx)
    }

    /// A folders list scoped to the immediate subdirectories of `path`.
    pub fn for_folder(path: &str, cx: &mut Context<Self>) -> Self {
        Self::scoped(Some(path.to_string().into()), cx)
    }

    fn scoped(scope: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<Services>();
        let library_event_bus = services.library_event_bus.clone();
        let lang_event_bus = services.lang_event_bus.clone();
        let library = services.library.clone();
        let is_scanning = library.is_scanning();

        let folders_all = Self::load(&library, scope.as_deref());
        let own_folder = Self::own_folder(&library, scope.as_deref());
        let sort = cx.global::<SettingsStore>().folder_sort();
        let layout = cx.global::<SettingsStore>().folder_layout();
        let library_access = library.library_access();

        let subscription =
            cx.subscribe(
                &library_event_bus,
                |this, _, event: &LibraryEvent, cx| match event {
                    LibraryEvent::ScanStarted => {
                        this.is_scanning = true;
                        cx.notify();
                    }
                    LibraryEvent::ScanComplete => {
                        this.is_scanning = false;
                        cx.notify();
                    }
                    LibraryEvent::CatalogChanged => {
                        this.reload_source(cx);
                        cx.notify();
                    }
                    _ => {}
                },
            );

        let lang_subscription = cx.subscribe(&lang_event_bus, |this, _, _: &LangChanged, cx| {
            this.recompute_visible();
            cx.notify();
        });

        // The sort and the layout live in the header, outside this view, so the
        // setting is the one place both can read. Changing either reorders or
        // re-shapes in place and jumps back to the top, where the new first
        // entry is.
        let settings_observer = cx.observe_global::<SettingsStore>(|this, cx| {
            let settings = cx.global::<SettingsStore>();
            let sort = settings.folder_sort();
            let layout = settings.folder_layout();
            if this.sort != sort || this.layout != layout {
                this.sort = sort;
                this.layout = layout;
                this.recompute_visible();
                this.scroll_handle
                    .scroll_to_item(0, gpui::ScrollStrategy::Top);
            }
            // The caption toggles do not change the geometry, but they do change
            // every tile, so the repaint happens either way.
            cx.notify();
        });

        let mut this = Self {
            scope,
            folders_all,
            rows: Vec::new(),
            own_folder,
            filter: String::new(),
            sort,
            layout,
            matcher: Matcher::new(Config::DEFAULT),
            is_scanning,
            item_sizes: Rc::new(Vec::new()),
            scroll_handle: VirtualListScrollHandle::new(),
            measured_width: px(0.),
            columns: 1,
            tile_width: 1.,
            rem_size: 16.,
            library_access,
            covers_in_flight: HashSet::new(),
            covers_unavailable: HashSet::new(),
            visible_span_peak: 0,
            _subscription: subscription,
            _lang_subscription: lang_subscription,
            _settings_observer: settings_observer,
        };
        this.recompute_visible();
        this
    }

    fn load(
        library: &crate::library_service::LibraryService,
        scope: Option<&str>,
    ) -> Vec<music_library::FolderSummary> {
        match scope {
            Some(path) => library.child_folders(path),
            None => library.folder_roots(),
        }
    }

    fn reload_source(&mut self, cx: &mut Context<Self>) {
        let library = cx.global::<Services>().library.clone();
        self.folders_all = Self::load(&library, self.scope.as_deref());
        self.own_folder = Self::own_folder(&library, self.scope.as_deref());
        self.recompute_visible();
    }

    /// The folder being listed, but only when it holds tracks of its own.
    ///
    /// `child_folders` lists subdirectories, so a folder with files *and*
    /// subdirectories would otherwise hide its own files behind the list of
    /// subdirectories. This is the row that keeps them.
    fn own_folder(
        library: &crate::library_service::LibraryService,
        scope: Option<&str>,
    ) -> Option<music_library::FolderSummary> {
        let path = scope?;
        let track_count = library.direct_track_count(path);
        if track_count == 0 {
            return None;
        }
        Some(music_library::FolderSummary {
            name: std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string()),
            path: path.to_string(),
            track_count,
            // Not shown on this row; the folder list above carries the count
            // that matters. Counting the albums here would mean another query
            // for a number nothing reads.
            album_count: 0,
            // This row opens a track list, so it is never drawn as a tile.
            cover_art_id: None,
            artist: None,
            year: None,
        })
    }

    fn build_rows(folders: &[music_library::FolderSummary]) -> Vec<FolderRow> {
        folders.iter().cloned().map(FolderRow::build).collect()
    }

    fn make_item_sizes(row_count: usize, has_own_row: bool) -> Vec<Size<Pixels>> {
        let mut sizes = vec![size(px(300.), px(TOP_PADDING))];
        if has_own_row {
            sizes.push(size(px(300.), px(FOLDER_ROW_HEIGHT + 1.)));
        }
        sizes.extend(vec![size(px(300.), px(FOLDER_ROW_HEIGHT + 1.)); row_count]);
        sizes
    }

    pub fn set_filter(&mut self, query: &str, cx: &mut Context<Self>) {
        let trimmed = query.trim().to_string();
        if trimmed == self.filter {
            return;
        }
        self.filter = trimmed;
        self.recompute_visible();
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn recompute_visible(&mut self) {
        let filtered: Vec<music_library::FolderSummary> = if self.filter.is_empty() {
            let mut folders = self.folders_all.clone();
            sort_folders(&mut folders, self.sort);
            folders
        } else {
            fuzzy_sorted(
                &mut self.matcher,
                &self.filter,
                self.folders_all
                    .iter()
                    .enumerate()
                    .map(|(ix, f)| (ix, f.name.as_str())),
            )
            .into_iter()
            .map(|ix| self.folders_all[ix].clone())
            .collect()
        };
        self.rows = Self::build_rows(&filtered);
        self.rebuild_items();
    }

    /// Recompute the virtual list's item heights for the current layout.
    fn rebuild_items(&mut self) {
        self.item_sizes = Rc::new(match self.layout {
            FolderLayout::List => {
                Self::make_item_sizes(self.rows.len(), self.own_folder.is_some())
            }
            FolderLayout::Grid => folders_grid::grid_item_sizes(
                self.rows.len(),
                self.own_folder.is_some(),
                self.columns,
                self.tile_width,
                self.rem_size,
            ),
        });
    }

    /// The panel width, from the grid's measuring canvas.
    ///
    /// Column count follows the window, so it cannot come from a setting. A
    /// width change rebuilds the rows; the caller repaints on the next frame.
    pub(super) fn set_grid_width(&mut self, width: Pixels) {
        self.measured_width = width;
        let (columns, tile_width) = albums_grid::grid_metrics(f32::from(width));
        self.columns = columns;
        self.tile_width = tile_width;
        if self.layout == FolderLayout::Grid {
            self.rebuild_items();
        }
    }

    /// Make sure the covers the visible tiles need are decoded.
    ///
    /// The grid asks for the visible rows plus one row of margin, so a scroll
    /// does not uncover a row of placeholders. The large-cover cache is shared
    /// with the album grid and bounded by what is on screen, which is what keeps
    /// a big window from holding a whole library of 320×320 pixels.
    pub(super) fn ensure_grid_covers(
        &mut self,
        folders: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let cache = cx.global::<Services>().cover_art_cache.clone();
        let capacity = capacity_for_peak_visible(&mut self.visible_span_peak, folders.len());
        cache.borrow_mut().set_large_capacity(capacity, cx);
        let mut wanted: Vec<i64> = {
            let cache = cache.borrow();
            folders
                .filter_map(|ix| self.rows.get(ix)?.summary.cover_art_id)
                .filter(|id| {
                    !cache.holds_large(*id)
                        && !self.covers_in_flight.contains(id)
                        && !self.covers_unavailable.contains(id)
                })
                .collect()
        };
        if wanted.is_empty() {
            return;
        }
        wanted.sort_unstable();
        wanted.dedup();
        for id in &wanted {
            self.covers_in_flight.insert(*id);
        }

        let access = self.library_access.clone();
        cx.spawn(async move |this, cx| {
            for id in wanted {
                let access = access.clone();
                let decoded = cx
                    .background_executor()
                    .spawn(async move {
                        access
                            .cover_large(id)
                            .and_then(|bytes| decode_cover_tile(&bytes))
                    })
                    .await;
                let updated = this.update(cx, |view, cx| {
                    view.covers_in_flight.remove(&id);
                    let Some(image) = decoded else {
                        view.covers_unavailable.insert(id);
                        return;
                    };
                    cx.global::<Services>()
                        .cover_art_cache
                        .clone()
                        .borrow_mut()
                        .insert_large(id, image, cx);
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

impl EventEmitter<FolderSelectedEvent> for FoldersView {}
impl EventEmitter<FolderTracksRequested> for FoldersView {}
impl EventEmitter<OpenLibrarySettings> for FoldersView {}

impl Render for FoldersView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = Colors::border(cx);
        let list_hover = Colors::list_hover(cx);
        let muted_fg = Colors::muted_foreground(cx);
        let accent = Colors::accent(cx);

        // Caption heights follow the window's font size, so a scale change has
        // to rebuild the tile rows. Reading it here keeps the two in step
        // without a second observer.
        let rem = f32::from(window.rem_size());
        if (self.rem_size - rem).abs() > 0.5 {
            self.rem_size = rem;
            self.rebuild_items();
        }

        if self.is_scanning && self.folders_all.is_empty() {
            return v_flex()
                .size_full()
                .child(div().px_4().child(tr().scanning.clone()));
        }

        // The own-tracks row is a row too: a folder whose subfolders are all it
        // has left, but which still holds files, must not read as empty.
        if self.rows.is_empty() && self.own_folder.is_none() {
            if self.folders_all.is_empty() {
                if !has_music_sources(cx) {
                    return empty_library(tr().no_music_sources.clone(), cx);
                }
                // At the top level an empty list means the library itself has
                // nothing; deeper in, it means this folder was emptied by a
                // rescan. "No music added yet" reads correctly for both.
                if self.scope.is_some() {
                    return v_flex().size_full().px_4().child(
                        div()
                            .text_color(muted_fg)
                            .child(tr().no_music_sources.clone()),
                    );
                }
                return v_flex().size_full().px_4().child(
                    div()
                        .text_color(muted_fg)
                        .child(tr().no_folders_found.clone()),
                );
            }
            return v_flex()
                .size_full()
                .gap_3()
                .child(div().px_4().child(tr().no_folders_match.clone()));
        }

        if self.layout == FolderLayout::Grid {
            return folders_grid::render_grid(self, cx);
        }

        let item_sizes = self.item_sizes.clone();
        let own_folder = self.own_folder.clone();
        v_flex()
            .size_full()
            .relative()
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    "folders_list",
                    item_sizes,
                    move |view, visible_range, _window, cx| {
                        visible_range
                            .map(|ix| {
                                if ix == 0 {
                                    return div().w_full().h(px(TOP_PADDING)).into_any_element();
                                }
                                let slot = ix - 1;
                                // Row 0 is the folder's own files when it has
                                // any; the subdirectory list follows it. `slot`
                                // short-circuits, so nothing is cloned for the
                                // rows that are not this one.
                                if slot == 0
                                    && let Some(own) = own_folder.as_ref()
                                {
                                    return own_row(own.clone(), cx);
                                }
                                let row_ix = if own_folder.is_some() { slot - 1 } else { slot };
                                let row = &view.rows[row_ix];

                                h_flex()
                                    .w_full()
                                    .h(px(FOLDER_ROW_HEIGHT))
                                    .px_4()
                                    .items_center()
                                    .gap_3()
                                    .border_b(px(1.))
                                    .border_color(border)
                                    .hover(|style| style.bg(list_hover))
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .with_size(px(18.))
                                            .flex_shrink_0()
                                            .text_color(accent),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(row.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted_fg)
                                            .child(row.count_label.clone()),
                                    )
                                    .id(ElementId::NamedInteger("folder-row".into(), row_ix as u64))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(row) = this.rows.get(row_ix) {
                                            cx.emit(FolderSelectedEvent {
                                                folder: row.summary.clone(),
                                            });
                                        }
                                    }))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.scroll_handle)
                .flex_1(),
            )
            .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical)
    }
}

/// The row that opens the tracks sitting directly in the folder being listed.
///
/// It is a full-width row in both layouts: it opens a track list rather than a
/// subdirectory, so it has no album art to be a tile.
pub(super) fn own_row(
    own: music_library::FolderSummary,
    cx: &mut Context<FoldersView>,
) -> AnyElement {
    let count_label = tr().n_tracks(own.track_count);
    let folder = own.clone();
    h_flex()
        .w_full()
        .h(px(FOLDER_ROW_HEIGHT))
        .px_4()
        .items_center()
        .gap_3()
        .border_b(px(1.))
        .border_color(Colors::border(cx))
        .hover(|style| style.bg(Colors::list_hover(cx)))
        .child(
            Icon::default()
                .path("icons/s1-tracks.svg")
                .with_size(px(18.))
                .flex_shrink_0()
                .text_color(Colors::accent(cx)),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .child(tr().tracks_in_this_folder.clone()),
        )
        .child(
            div()
                .text_sm()
                .text_color(Colors::muted_foreground(cx))
                .child(count_label),
        )
        .id(ElementId::Name("folder-row-own".into()))
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.emit(FolderTracksRequested {
                folder: folder.clone(),
            });
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, tracks: i64) -> music_library::FolderSummary {
        music_library::FolderSummary {
            name: name.to_string(),
            path: format!("/music/{name}"),
            track_count: tracks,
            album_count: 0,
            cover_art_id: None,
            artist: None,
            year: None,
        }
    }

    fn sorted(sort: FolderSort) -> Vec<String> {
        let mut folders = vec![
            folder("beta", 3),
            folder("Alpha", 10),
            folder("gamma", 3),
            folder("delta", 1),
        ];
        sort_folders(&mut folders, sort);
        folders.into_iter().map(|folder| folder.name).collect()
    }

    #[test]
    fn name_sorts_ignore_case() {
        assert_eq!(sorted(FolderSort::Name), ["Alpha", "beta", "delta", "gamma"]);
        assert_eq!(
            sorted(FolderSort::NameDesc),
            ["gamma", "delta", "beta", "Alpha"]
        );
    }

    #[test]
    fn track_sorts_keep_a_name_order_on_ties() {
        // `beta` and `gamma` both hold three tracks, so the tie-break decides:
        // a scan that returns them the other way round must not reshuffle them.
        assert_eq!(
            sorted(FolderSort::Tracks),
            ["delta", "beta", "gamma", "Alpha"]
        );
        assert_eq!(
            sorted(FolderSort::TracksDesc),
            ["Alpha", "beta", "gamma", "delta"]
        );
    }
}
