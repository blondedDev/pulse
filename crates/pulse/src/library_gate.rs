//! The window that opens when the library database cannot be opened.
//!
//! Opening the database used to be an `expect`, which meant a corrupt file, an
//! unmounted disk or a second copy of Pulse holding the lock killed the process
//! before anything was drawn. The user saw nothing at all. This is the same
//! shape as `pipewire_alsa_gate`: one window, what went wrong, and the two
//! things worth doing about it.

use gpui::{
    App, AppContext, Context, FontWeight, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div, px,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    v_flex,
};

use crate::localization::tr;
use crate::theme_colors::Colors;

pub fn open(cx: &mut App, error: String) {
    let options = crate::build_window_options(cx);
    cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| LibraryGate::new(error, window, cx));
        cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("Failed to open the library gate window");
}

pub struct LibraryGate {
    error: SharedString,
    data_dir: Option<std::path::PathBuf>,
    backups_dir: Option<std::path::PathBuf>,
}

impl LibraryGate {
    fn new(error: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let data_dir = crate::paths::data_dir();
        let backups_dir = data_dir.as_ref().map(|dir| dir.join("backups"));
        Self {
            error: error.into(),
            data_dir,
            backups_dir,
        }
    }
}

fn path_row(
    label: SharedString,
    path: Option<std::path::PathBuf>,
    id: &'static str,
    cx: &App,
) -> impl IntoElement {
    let muted = Colors::muted_foreground(cx);
    let shown: SharedString = match &path {
        Some(path) => path.display().to_string().into(),
        None => tr().not_available.clone(),
    };
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .child(div().text_xs().text_color(muted).child(label))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .text_ellipsis()
                .text_sm()
                .text_color(Colors::foreground(cx))
                .child(shown),
        )
        .child(
            Button::new(id)
                .small()
                .label(tr().reveal_folder.clone())
                .disabled(path.is_none())
                .on_click(move |_, _, _| {
                    if let Some(path) = &path {
                        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
                    }
                }),
        )
}

impl Render for LibraryGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = tr();
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(Colors::title_bar(cx))
            .child(
                div().size_full().flex().items_center().justify_center().p_4().child(
                    v_flex()
                        .w_full()
                        .max_w(px(620.))
                        .gap_5()
                        .p_8()
                        .rounded(px(12.))
                        .bg(Colors::background(cx))
                        .border_1()
                        .border_color(Colors::border(cx))
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(Colors::foreground(cx))
                                        .child(s.library_open_failed_title.clone()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(Colors::muted_foreground(cx))
                                        .child(s.library_open_failed_body.clone()),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .w_full()
                                .px_3()
                                .py_2()
                                .rounded(px(6.))
                                .bg(Colors::muted(cx))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(Colors::foreground(cx))
                                        .child(self.error.clone()),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .w_full()
                                .child(path_row(
                                    s.library_open_failed_database.clone(),
                                    self.data_dir.clone(),
                                    "library-gate-data",
                                    cx,
                                ))
                                .child(path_row(
                                    s.library_open_failed_backups.clone(),
                                    self.backups_dir.clone(),
                                    "library-gate-backups",
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(Colors::muted_foreground(cx))
                                .child(s.library_open_failed_hint.clone()),
                        )
                        .child(
                            h_flex().justify_end().child(
                                Button::new("library-gate-quit")
                                    .primary()
                                    .small()
                                    .label(s.quit_pulse.clone())
                                    .on_click(|_, _, cx| cx.quit()),
                            ),
                        ),
                ),
            )
    }
}
