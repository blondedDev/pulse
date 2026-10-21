//! Exposes [`tr`], the entry point every view uses to read UI strings.
//!
//! Pulse ships **English only**, so there is no language resolution left. The
//! tables live in the `ui_resources` crate (the UI data layer); [`tr`] just
//! hands back the one static table, allocation-free.

use gpui::EventEmitter;
use ui_resources::i18n::Strings;

/// Emitted app-wide when the active UI language changes.
///
/// Kept as a type because several views subscribe to it while caching
/// localized strings off the render hot path. With a single language it is
/// never emitted, but the subscription pattern still compiles against it.
pub struct LangChanged;

/// Tiny event bus carrying [`LangChanged`]. Stored on `Services` so any view
/// can subscribe. Mirrors the other per-domain buses (library, engine).
pub struct LangEventBus;
impl EventEmitter<LangChanged> for LangEventBus {}

/// The UI string table — the per-label hot path.
pub fn tr() -> &'static Strings {
    ui_resources::i18n::strings()
}
