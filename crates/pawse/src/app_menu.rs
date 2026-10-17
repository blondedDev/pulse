use gpui::{App, Menu, MenuItem, actions};

use crate::localization::tr;

actions!(pawse, [Quit]);

pub fn app_menus() -> Vec<Menu> {
    let s = tr();
    vec![Menu {
        name: "Pulse".into(),
        items: vec![MenuItem::action(s.quit_pulse.clone(), Quit)],
        disabled: false,
    }]
}

pub fn set_menus(cx: &App) {
    cx.set_menus(app_menus());
}
