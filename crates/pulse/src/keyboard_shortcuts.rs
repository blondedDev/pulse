use gpui::{App, KeyBinding, actions};

actions!(
    pulse,
    [
        SeekForward,
        SeekBackward,
        NextTrack,
        PreviousTrack,
        VolumeUp,
        VolumeDown,
        PlayPause,
        ExitCoverMode,
        ToggleCoverMode,
        PlayNextHovered,
    ]
);

pub const CONTEXT: &str = "MainView";

pub(crate) const BINDING: &str = "MainView && !Input";

pub fn init(cx: &mut App) {
    let (next, prev) = ("ctrl-right", "ctrl-left");

    cx.bind_keys([
        KeyBinding::new("right", SeekForward, Some(BINDING)),
        KeyBinding::new("left", SeekBackward, Some(BINDING)),
        KeyBinding::new(next, NextTrack, Some(BINDING)),
        KeyBinding::new(prev, PreviousTrack, Some(BINDING)),
        KeyBinding::new("up", VolumeUp, Some(BINDING)),
        KeyBinding::new("down", VolumeDown, Some(BINDING)),
        KeyBinding::new("space", PlayPause, Some(BINDING)),
        KeyBinding::new("escape", ExitCoverMode, Some(BINDING)),
        // Cover view is a layer, not a destination: the same key takes it away
        // and leaves you on the screen you opened it from.
        KeyBinding::new("f", ToggleCoverMode, Some(BINDING)),
        // On the row under the cursor, like the hover buttons do.
        KeyBinding::new("q", PlayNextHovered, Some(BINDING)),
    ]);

}
