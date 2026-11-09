use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderImage, SharedString, StatefulInteractiveElement, Styled, canvas, div, px,
};
use gpui_component::{
    h_flex,
    scroll::{ScrollableElement, ScrollbarAxis},
    v_flex, v_virtual_list,
};
use ui_components::cover_thumb::cover_tile;

use crate::library_views::albums_grid::{caption_heights, row_height};
use crate::library_views::folders_view::{
    FOLDER_ROW_HEIGHT, FolderSelectedEvent, FoldersView, TOP_PADDING, own_row,
};
use crate::localization::tr;
use crate::services::Services;
use crate::settings_store::SettingsStore;
use crate::theme_colors::Colors;

const GRID_PAD_X: f32 = 16.;
const TILE_GAP: f32 = 16.;
const TILE_PAD: f32 = 8.;
const TILE_TEXT_GAP: f32 = 4.;
const TILE_RADIUS: f32 = 6.;
/// How many covers a folder tile draws in a collage.
pub(super) const COLLAGE_COVERS: usize = 4;
/// The gap between collage cells. Two pixels reads as a seam rather than a
/// layout, which is what a quartered picture wants.
const COLLAGE_GAP: f32 = 2.;

/// Choose the covers a folder tile draws.
///
/// Fewer than four candidates keep a single cover: a 2×2 of one picture is just
/// a smaller picture. Four or more fill a collage with distinct albums, so the
/// tile says "this folder holds several albums" instead of always looking like
/// whichever album happens to own the most tracks. The choice is made when the
/// rows are rebuilt, not on every paint, so the grid is stable while it is on
/// screen and refreshes when the folder, sort, layout or catalog changes.
pub(super) fn pick_tile_covers(covers: &[i64], rng: &mut impl rand::Rng) -> Vec<i64> {
    if covers.is_empty() {
        return Vec::new();
    }
    if covers.len() < COLLAGE_COVERS {
        let ix = rng.random_range(0..covers.len());
        return vec![covers[ix]];
    }
    // Partial Fisher–Yates: four distinct picks in the order they were drawn,
    // so the collage is not always laid out in contribution order.
    let mut pool = covers.to_vec();
    for i in 0..COLLAGE_COVERS {
        let j = rng.random_range(i..pool.len());
        pool.swap(i, j);
    }
    pool.truncate(COLLAGE_COVERS);
    pool
}

struct TileParams {
    columns: usize,
    tile_width: f32,
    cover_size: f32,
    title_height: f32,
    subtitle_height: f32,
    list_hover: Hsla,
    muted: Hsla,
    muted_fg: Hsla,
    show_cover: bool,
    show_artist: bool,
    show_year: bool,
}

/// What a tile says under the folder's name.
///
/// The album's artist and year are the two fields a music folder usually
/// answers to, but a folder of loose files has neither. Falling back to the
/// track count keeps every tile the same height and still says something,
/// instead of leaving a blank line.
pub(super) fn tile_subtitle(
    folder: &music_library::FolderSummary,
    show_artist: bool,
    show_year: bool,
) -> SharedString {
    let mut parts: Vec<String> = Vec::new();
    if show_artist && let Some(artist) = folder.artist.as_ref() {
        parts.push(artist.clone());
    }
    if show_year && let Some(year) = folder.year {
        parts.push(year.to_string());
    }
    if parts.is_empty() {
        tr().n_tracks(folder.track_count).into()
    } else {
        parts.join(" \u{00b7} ").into()
    }
}

pub(super) fn render_grid(view: &mut FoldersView, cx: &mut Context<FoldersView>) -> Div {
    let measure = {
        let entity = cx.entity();
        canvas(
            move |bounds, window, cx| {
                let width = bounds.size.width;
                if entity.read(cx).measured_width != width {
                    entity.update(cx, |this, _| this.set_grid_width(width));
                    window.on_next_frame(move |_, cx| {
                        entity.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    };

    let container = v_flex().size_full().relative().child(measure);
    if view.measured_width <= px(0.) {
        return container;
    }

    let params = {
        let settings = cx.global::<SettingsStore>();
        let (title_height, subtitle_height) = caption_heights(view.rem_size);
        TileParams {
            columns: view.columns.max(1),
            tile_width: view.tile_width,
            cover_size: (view.tile_width - 2. * TILE_PAD).max(1.),
            title_height,
            subtitle_height,
            list_hover: Colors::list_hover(cx),
            muted: Colors::secondary(cx),
            muted_fg: Colors::muted_foreground(cx),
            show_cover: settings.folders_show_cover(),
            show_artist: settings.folders_show_artist(),
            show_year: settings.folders_show_year(),
        }
    };
    let item_sizes = view.item_sizes.clone();
    let own_folder = view.own_folder.clone();
    let own_row_offset = usize::from(own_folder.is_some());

    container
        .child(
            v_virtual_list(
                cx.entity().clone(),
                "folders_grid",
                item_sizes,
                move |view, visible_range, _window, cx| {
                    if visible_range.end > 1 + own_row_offset {
                        let margin = params.columns;
                        let first =
                            visible_range.start.saturating_sub(1 + own_row_offset) * params.columns;
                        let last =
                            visible_range.end.saturating_sub(1 + own_row_offset) * params.columns;
                        view.ensure_grid_covers(
                            first.saturating_sub(margin)..(last + margin).min(view.rows.len()),
                            cx,
                        );
                    }
                    visible_range
                        .map(|ix| {
                            if ix == 0 {
                                return div().w_full().h(px(TOP_PADDING)).into_any_element();
                            }
                            // The folder's own files stay a full-width row: it
                            // opens a track list rather than a folder, so it has
                            // no album to look like.
                            if own_row_offset == 1
                                && ix == 1
                                && let Some(own) = own_folder.as_ref()
                            {
                                return own_row(own.clone(), cx);
                            }
                            grid_row(view, ix - 1 - own_row_offset, &params, cx)
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&view.scroll_handle)
            .overflow_x_hidden()
            .flex_1(),
        )
        .scrollbar(&view.scroll_handle, ScrollbarAxis::Vertical)
}

fn grid_row(
    view: &mut FoldersView,
    row_ix: usize,
    p: &TileParams,
    cx: &mut Context<FoldersView>,
) -> AnyElement {
    let start = row_ix * p.columns;
    let end = (start + p.columns).min(view.rows.len());
    if start >= end {
        return div().into_any_element();
    }
    let covers: Vec<Vec<Option<Arc<RenderImage>>>> = if p.show_cover {
        let cache = cx.global::<Services>().cover_art_cache.clone();
        let mut cache = cache.borrow_mut();
        (start..end)
            .map(|ix| {
                view.rows[ix]
                    .tile_covers
                    .iter()
                    .map(|id| cache.peek_large(Some(*id)))
                    .collect()
            })
            .collect()
    } else {
        vec![Vec::new(); end - start]
    };
    let mut row = h_flex()
        .w_full()
        .px(px(GRID_PAD_X))
        .gap(px(TILE_GAP))
        .items_start();
    for (slot, ix) in (start..end).enumerate() {
        row = row.child(grid_tile(view, ix, &covers[slot], p, cx));
    }
    row.into_any_element()
}

fn grid_tile(
    view: &FoldersView,
    ix: usize,
    covers: &[Option<Arc<RenderImage>>],
    p: &TileParams,
    cx: &mut Context<FoldersView>,
) -> AnyElement {
    let folder = view.rows[ix].summary.clone();
    let name = view.rows[ix].name.clone();
    let subtitle = tile_subtitle(&folder, p.show_artist, p.show_year);

    let mut tile = v_flex()
        .w(px(p.tile_width))
        .flex_shrink_0()
        .p(px(TILE_PAD))
        .gap(px(TILE_TEXT_GAP))
        .rounded(px(TILE_RADIUS + TILE_PAD))
        .hover(|style| style.bg(p.list_hover));
    if p.show_cover {
        tile = tile.child(tile_cover(covers, p));
    }
    tile.child(
        div()
            .w_full()
            .h(px(p.title_height))
            .line_height(px(p.title_height))
            .overflow_hidden()
            .text_ellipsis()
            .text_sm()
            .child(name),
    )
    .child(
        div()
            .w_full()
            .h(px(p.subtitle_height))
            .line_height(px(p.subtitle_height))
            .overflow_hidden()
            .text_ellipsis()
            .text_xs()
            .text_color(p.muted_fg)
            .child(subtitle),
    )
    .id(ElementId::NamedInteger("folder-tile".into(), ix as u64))
    .on_click(cx.listener(move |_, _, _, cx| {
        cx.emit(FolderSelectedEvent {
            folder: folder.clone(),
        });
    }))
    .into_any_element()
}

/// The picture area of a tile: one cover, or a 2×2 collage when the row drew
/// four.
fn tile_cover(covers: &[Option<Arc<RenderImage>>], p: &TileParams) -> AnyElement {
    if covers.len() >= COLLAGE_COVERS {
        return collage(covers, p);
    }
    cover_tile(
        covers.first().and_then(|cover| cover.as_ref()),
        p.cover_size,
        TILE_RADIUS,
        p.muted,
        p.muted_fg,
    )
}

/// The side of one collage cell, for a square picture area.
fn collage_cell(cover_size: f32) -> f32 {
    ((cover_size - COLLAGE_GAP) / 2.).max(1.)
}

/// Four covers in a 2×2, with a hairline seam so they read as four albums
/// rather than one strange picture.
fn collage(covers: &[Option<Arc<RenderImage>>], p: &TileParams) -> AnyElement {
    let cell = collage_cell(p.cover_size);
    let radius = TILE_RADIUS / 2.;
    let mut grid = v_flex()
        .w(px(p.cover_size))
        .h(px(p.cover_size))
        .gap(px(COLLAGE_GAP));
    for pair in covers.chunks(2).take(2) {
        let mut line = h_flex().gap(px(COLLAGE_GAP));
        for cover in pair {
            line = line.child(cover_tile(
                cover.as_ref(),
                cell,
                radius,
                p.muted,
                p.muted_fg,
            ));
        }
        grid = grid.child(line);
    }
    grid.into_any_element()
}

/// Tile rows for a list of `row_count` folders, plus the padding and the
/// own-files row that sit above them.
pub(super) fn grid_item_sizes(
    row_count: usize,
    has_own_row: bool,
    columns: usize,
    tile_width: f32,
    rem: f32,
) -> Vec<gpui::Size<gpui::Pixels>> {
    let mut sizes = vec![gpui::size(px(300.), px(TOP_PADDING))];
    if has_own_row {
        sizes.push(gpui::size(px(300.), px(FOLDER_ROW_HEIGHT + 1.)));
    }
    let tile_rows = row_count.div_ceil(columns.max(1));
    let height = row_height(tile_width, rem);
    sizes.extend(vec![gpui::size(px(300.), px(height)); tile_rows]);
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng as _;

    fn folder(
        name: &str,
        artist: Option<&str>,
        year: Option<i32>,
        tracks: i64,
    ) -> music_library::FolderSummary {
        music_library::FolderSummary {
            name: name.to_string(),
            path: format!("/music/{name}"),
            track_count: tracks,
            album_count: 1,
            cover_art_ids: Vec::new(),
            artist: artist.map(str::to_string),
            year,
        }
    }

    #[test]
    fn the_caption_joins_the_fields_that_are_on() {
        let f = folder("Kid A", Some("Radiohead"), Some(2000), 10);
        assert_eq!(tile_subtitle(&f, true, true), "Radiohead · 2000");
        assert_eq!(tile_subtitle(&f, true, false), "Radiohead");
        assert_eq!(tile_subtitle(&f, false, true), "2000");
    }

    #[test]
    fn a_caption_with_nothing_to_say_counts_the_tracks() {
        let loose = folder("Loose", None, None, 7);
        assert_eq!(tile_subtitle(&loose, true, true), "7 tracks");
        // Hiding both fields leaves the count, not an empty line.
        let tagged = folder("Kid A", Some("Radiohead"), Some(2000), 10);
        assert_eq!(tile_subtitle(&tagged, false, false), "10 tracks");
    }

    #[test]
    fn a_row_is_only_as_tall_as_the_tiles_it_holds() {
        let sizes = grid_item_sizes(5, false, 2, 160., 16.);
        assert_eq!(sizes.len(), 1 + 3, "padding plus three tile rows");

        let with_own = grid_item_sizes(5, true, 2, 160., 16.);
        assert_eq!(with_own.len(), 1 + 1 + 3);
        assert_eq!(
            f32::from(with_own[1].height),
            FOLDER_ROW_HEIGHT + 1.,
            "the own-files row keeps its list height"
        );
    }

    #[test]
    fn an_empty_grid_is_just_the_padding() {
        assert_eq!(grid_item_sizes(0, false, 3, 120., 16.).len(), 1);
    }

    #[test]
    fn fewer_than_four_covers_keep_one() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        for len in 1..COLLAGE_COVERS {
            let covers: Vec<i64> = (0..len as i64).collect();
            let picked = pick_tile_covers(&covers, &mut rng);
            assert_eq!(picked.len(), 1, "{len} candidates still show one cover");
            assert!(covers.contains(&picked[0]));
        }
        assert!(pick_tile_covers(&[], &mut rng).is_empty());
    }

    #[test]
    fn four_or_more_covers_fill_a_collage_with_distinct_albums() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let covers: Vec<i64> = (0..6).collect();
        let picked = pick_tile_covers(&covers, &mut rng);
        assert_eq!(picked.len(), COLLAGE_COVERS);
        let unique: std::collections::HashSet<i64> = picked.iter().copied().collect();
        assert_eq!(unique.len(), COLLAGE_COVERS, "no album shown twice");
        assert!(picked.iter().all(|id| covers.contains(id)));
    }

    #[test]
    fn the_same_seed_picks_the_same_covers() {
        let covers: Vec<i64> = (0..8).collect();
        let a = pick_tile_covers(&covers, &mut rand::rngs::StdRng::seed_from_u64(3));
        let b = pick_tile_covers(&covers, &mut rand::rngs::StdRng::seed_from_u64(3));
        assert_eq!(a, b);
    }

    #[test]
    fn collage_cells_fill_the_picture_area() {
        for size in [120., 160., 200.] {
            let cell = collage_cell(size);
            assert!(
                (cell * 2. + COLLAGE_GAP - size).abs() < 0.01,
                "two cells and the seam should exactly span {size}"
            );
        }
        assert_eq!(collage_cell(1.), 1., "a tiny tile still gets a visible cell");
    }

    /// The picture area has to build for both shapes without a window: this is
    /// the one place the grid touches elements, and a panic here would take the
    /// whole page down.
    #[test]
    fn both_tile_shapes_build() {
        let p = TileParams {
            columns: 4,
            tile_width: 168.,
            cover_size: 152.,
            title_height: 20.,
            subtitle_height: 18.,
            list_hover: gpui::black(),
            muted: gpui::black(),
            muted_fg: gpui::black(),
            show_cover: true,
            show_artist: true,
            show_year: true,
        };
        let _single = tile_cover(&[], &p);
        let _collage = tile_cover(&[None, None, None, None], &p);
    }
}
