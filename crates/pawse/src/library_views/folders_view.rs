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
        let item_sizes = Self::make_item_sizes(rows.len());

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
        self.folders_all = Self::load(&cx.global::<Services>().library, self.scope.as_deref());
        self.recompute_visible();
    }

    fn build_rows(folders: &[music_library::FolderSummary]) -> Vec<FolderRow> {
        folders.iter().cloned().map(FolderRow::build).collect()
    }

    fn make_item_sizes(row_count: usize) -> Rc<Vec<Size<Pixels>>> {
        let mut sizes = vec![size(px(300.), px(TOP_PADDING))];
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
        self.item_sizes = Self::make_item_sizes(self.rows.len());
    }
}

impl EventEmitter<FolderSelectedEvent> for FoldersView {}
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

        if self.rows.is_empty() {
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
                                let row_ix = ix - 1;
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
