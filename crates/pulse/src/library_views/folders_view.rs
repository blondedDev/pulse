use std::rc::Rc;

use gpui::{
    Context, ElementId, EventEmitter, InteractiveElement, IntoElement, ParentElement, Pixels,
    Render, SharedString, Size, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
    size,
};
use gpui_component::{
    Icon, IconName, Sizable, VirtualListScrollHandle, h_flex,
    scroll::{ScrollableElement, ScrollbarAxis},
    v_flex, v_virtual_list,
};
use nucleo_matcher::{Config, Matcher};

use crate::library_service::LibraryEvent;
use crate::library_views::albums_view::{OpenLibrarySettings, empty_library, has_music_sources};
use crate::library_views::fuzzy::fuzzy_sorted;
use crate::localization::{LangChanged, tr};
use crate::services::Services;
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

struct FolderRow {
    summary: music_library::FolderSummary,
    name: SharedString,
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

const TOP_PADDING: f32 = 12.;
const FOLDER_ROW_HEIGHT: f32 = 44.;

pub struct FoldersView {
    /// `None` lists the configured library roots; `Some(path)` lists the
    /// immediate subdirectories of that folder instead. This is what makes the
    /// tab drill down one level at a time.
    scope: Option<SharedString>,
    folders_all: Vec<music_library::FolderSummary>,
    rows: Vec<FolderRow>,
    /// The folder being listed, when it has tracks sitting directly in it.
    /// Rendered as one extra row above the subdirectories so those tracks stay
    /// reachable; `None` when there are none, which is the common case.
    own_folder: Option<music_library::FolderSummary>,
    filter: String,
    matcher: Matcher,
    is_scanning: bool,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
    _subscription: Subscription,
    _lang_subscription: Subscription,
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
        let rows = Self::build_rows(&folders_all);
        let own_folder = Self::own_folder(&library, scope.as_deref());
        let item_sizes = Self::make_item_sizes(rows.len(), own_folder.is_some());

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

        Self {
            scope,
            folders_all,
            rows,
            own_folder,
            filter: String::new(),
            matcher: Matcher::new(Config::DEFAULT),
            is_scanning,
            item_sizes,
            scroll_handle: VirtualListScrollHandle::new(),
            _subscription: subscription,
            _lang_subscription: lang_subscription,
        }
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
        })
    }

    fn build_rows(folders: &[music_library::FolderSummary]) -> Vec<FolderRow> {
        folders.iter().cloned().map(FolderRow::build).collect()
    }

    fn make_item_sizes(row_count: usize, has_own_row: bool) -> Rc<Vec<Size<Pixels>>> {
        let mut sizes = vec![size(px(300.), px(TOP_PADDING))];
        if has_own_row {
            sizes.push(size(px(300.), px(FOLDER_ROW_HEIGHT + 1.)));
        }
        sizes.extend(vec![size(px(300.), px(FOLDER_ROW_HEIGHT + 1.)); row_count]);
        Rc::new(sizes)
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
            self.folders_all.clone()
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
        self.item_sizes = Self::make_item_sizes(self.rows.len(), self.own_folder.is_some());
    }
}

impl EventEmitter<FolderSelectedEvent> for FoldersView {}
impl EventEmitter<FolderTracksRequested> for FoldersView {}
impl EventEmitter<OpenLibrarySettings> for FoldersView {}

impl Render for FoldersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = Colors::border(cx);
        let list_hover = Colors::list_hover(cx);
        let muted_fg = Colors::muted_foreground(cx);
        let accent = Colors::accent(cx);

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
                                    let count_label = tr().n_tracks(own.track_count);
                                    let own = own.clone();
                                    return h_flex()
                                        .w_full()
                                        .h(px(FOLDER_ROW_HEIGHT))
                                        .px_4()
                                        .items_center()
                                        .gap_3()
                                        .border_b(px(1.))
                                        .border_color(border)
                                        .hover(|style| style.bg(list_hover))
                                        .child(
                                            Icon::default()
                                                .path("icons/s1-tracks.svg")
                                                .with_size(px(18.))
                                                .flex_shrink_0()
                                                .text_color(accent),
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
                                                .text_color(muted_fg)
                                                .child(count_label),
                                        )
                                        .id(ElementId::Name("folder-row-own".into()))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(FolderTracksRequested { folder: own.clone() });
                                        }))
                                        .into_any_element();
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
