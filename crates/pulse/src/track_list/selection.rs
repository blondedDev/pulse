//! Row selection for the track lists.
//!
//! A plain click plays, as it always has. Shift-click takes the range from the
//! last clicked row, and Ctrl/Cmd-click toggles one row — neither starts
//! playback, because the point of selecting is to act on several rows at once.
//!
//! The rules live here rather than in each view so the three lists behave
//! identically, and so they can be tested without a window.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Hsla, IntoElement, Modifiers, ParentElement, SharedString,
    Styled, Window, div, point, px,
};
use gpui_component::{
    Sizable,
    button::Button,
    h_flex,
};

use crate::localization::tr;
use crate::playlist_popup::OpenAddToPlaylist;
use crate::services::Services;
use crate::theme_colors::Colors;

/// What a click on a track row should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickAction {
    /// Play this row, the plain-click behaviour.
    Play,
    /// The click only changed the selection; do not touch playback.
    Selection,
}

#[derive(Default)]
pub struct Selection {
    /// The row a shift-click extends from.
    anchor: Option<usize>,
    ids: HashSet<i64>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(&id)
    }

    /// Drop the selection. Returns whether anything was selected.
    pub fn clear(&mut self) -> bool {
        self.anchor = None;
        !self.ids.drain().next().is_none()
    }

    /// Decide what a click on row `index` means.
    ///
    /// `ids` is the list in display order, which is what a shift range is
    /// measured against. A plain click also clears the selection: the user has
    /// chosen to play something, so the old selection is no longer what they
    /// are looking at.
    pub fn click(&mut self, ids: &[i64], index: usize, modifiers: &Modifiers) -> ClickAction {
        if index >= ids.len() {
            return ClickAction::Play;
        }

        if modifiers.shift {
            // A shift-click with nothing selected starts a selection rather
            // than doing nothing.
            let anchor = self.anchor.unwrap_or(index);
            let (first, last) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            self.ids.clear();
            for id in &ids[first..=last] {
                self.ids.insert(*id);
            }
            // The anchor stays where it was, so dragging the range back and
            // forth with repeated shift-clicks works like a text editor.
            self.anchor = Some(anchor);
            return ClickAction::Selection;
        }

        if modifiers.secondary() || modifiers.control {
            let id = ids[index];
            if !self.ids.remove(&id) {
                self.ids.insert(id);
            }
            self.anchor = Some(index);
            return ClickAction::Selection;
        }

        self.ids.clear();
        self.anchor = Some(index);
        ClickAction::Play
    }

    /// The selected ids in display order.
    pub fn ids_in_order(&self, ids: &[i64]) -> Vec<i64> {
        ids.iter()
            .copied()
            .filter(|id| self.ids.contains(id))
            .collect()
    }
}

/// The row highlight for a selected track.
pub fn selected_row_bg(cx: &App) -> Hsla {
    Colors::secondary(cx).opacity(0.7)
}

/// The bar that appears above a list while rows are selected.
///
/// It acts on the selection directly, so every list gets the same two actions
/// and the same wording.
pub fn selection_bar(
    selected: Vec<Rc<music_library::Track>>,
    on_clear: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let count = selected.len();
    let for_queue = selected.clone();
    let for_playlist = selected;

    h_flex()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .bg(Colors::secondary(cx))
        .child(
            div()
                .text_sm()
                .child(SharedString::from(tr().selection_count(count))),
        )
        .child(div().flex_1())
        .child(
            Button::new("selection-add-to-queue")
                .small()
                .label(tr().add_to_queue.clone())
                .on_click(move |_, _, cx| {
                    crate::track_list::append_tracks_to_queue(for_queue.clone(), cx);
                }),
        )
        .child(
            Button::new("selection-add-to-playlist")
                .small()
                .label(tr().add_to_playlist.clone())
                .on_click(move |event: &ClickEvent, _, cx| {
                    let position = event.position();
                    let anchor = point(position.x - px(260.), position.y + px(8.));
                    let track_ids: Vec<i64> = for_playlist.iter().map(|track| track.id).collect();
                    let bus = cx.global::<Services>().playlist_popup_bus.clone();
                    bus.update(cx, |_, cx| cx.emit(OpenAddToPlaylist { track_ids, anchor }));
                }),
        )
        .child(
            Button::new("selection-clear")
                .small()
                .label(tr().clear_selection.clone())
                .on_click(on_clear),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Modifiers {
        Modifiers::default()
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    const IDS: [i64; 5] = [10, 11, 12, 13, 14];

    #[test]
    fn a_plain_click_plays_and_clears() {
        let mut selection = Selection::default();
        assert_eq!(
            selection.click(&IDS, 2, &ctrl()),
            ClickAction::Selection
        );
        assert_eq!(selection.len(), 1);

        assert_eq!(selection.click(&IDS, 1, &plain()), ClickAction::Play);
        assert!(selection.is_empty());
    }

    #[test]
    fn ctrl_click_toggles_one_row() {
        let mut selection = Selection::default();
        selection.click(&IDS, 1, &ctrl());
        selection.click(&IDS, 3, &ctrl());
        assert_eq!(selection.ids_in_order(&IDS), vec![11, 13]);

        // Clicking it again removes it.
        selection.click(&IDS, 1, &ctrl());
        assert_eq!(selection.ids_in_order(&IDS), vec![13]);
    }

    #[test]
    fn shift_click_takes_the_range_from_the_anchor() {
        let mut selection = Selection::default();
        selection.click(&IDS, 1, &ctrl());
        assert_eq!(selection.click(&IDS, 3, &shift()), ClickAction::Selection);
        assert_eq!(selection.ids_in_order(&IDS), vec![11, 12, 13]);

        // Dragging the range back over the anchor narrows it again.
        selection.click(&IDS, 2, &shift());
        assert_eq!(selection.ids_in_order(&IDS), vec![11, 12]);

        // And above the anchor measures backwards.
        selection.click(&IDS, 0, &shift());
        assert_eq!(selection.ids_in_order(&IDS), vec![10, 11]);
    }

    #[test]
    fn shift_without_a_previous_click_selects_that_row() {
        let mut selection = Selection::default();
        assert_eq!(selection.click(&IDS, 2, &shift()), ClickAction::Selection);
        assert_eq!(selection.ids_in_order(&IDS), vec![12]);
    }

    #[test]
    fn an_out_of_range_index_is_ignored() {
        let mut selection = Selection::default();
        assert_eq!(selection.click(&IDS, 99, &ctrl()), ClickAction::Play);
        assert!(selection.is_empty());
    }

    #[test]
    fn selected_ids_come_back_in_display_order_not_click_order() {
        let mut selection = Selection::default();
        selection.click(&IDS, 4, &ctrl());
        selection.click(&IDS, 0, &ctrl());
        assert_eq!(selection.ids_in_order(&IDS), vec![10, 14]);
    }
}
