# ui_resources

The UI **data layer**: resources the UI consumes but that are not UI logic.
Extracted from the `pulse` crate so that binary stays focused on views and app
state.

## Files

- `lib.rs` — re-exports the asset and i18n modules.
- `assets.rs` — `Assets` (`rust_embed`), the `gpui::AssetSource` that embeds the
  icon SVGs from `../assets/icons/**`. Passed to `Application::new().with_assets(...)`
  in `pulse::main`. The `#[folder = "assets"]` path is relative to this crate's
  `CARGO_MANIFEST_DIR`, i.e. `crates/ui_resources/assets/`.
- `i18n/` — the English string table (see below).

## Data folders

- `assets/` — `icons/` (embedded) and `app-icon/` (the PNG set
  `cargo-packager` reads; the paths are referenced from
  `crates/pulse/Cargo.toml` as `../ui_resources/assets/...`). `gen-icons.sh`
  regenerates them from `web/public/pulse.svg`.

## Theme

There is no theme module and no bundled theme files. Pulse 3.0 ships exactly one
look: the `gpui-component` **Default Dark** theme, applied at startup by
`pulse::main`'s `apply_base_theme`. There is no theme picker and no `ThemeChoice`;
the old `themes/` directory and its 21 JSON files are gone. The only theme-related
setting left is the adaptive "dim the backdrop to the cover art" behaviour.

## i18n module

A single compile-time-checked string table: **English only**.

- `i18n/mod.rs` — the `Strings` schema (one `SharedString` field per UI label),
  the `lang!` macro, and the interpolation accessor methods on `impl Strings`.
- `i18n/en.rs` — `pub static EN: Strings = lang!{ ... };`.
- `strings()` returns `&'static EN`. `tr()` lives in `pulse` (`localization.rs`)
  and is the only thing call sites use.

There is no `Lang` enum, no `lang` setting, no language picker and no
`languages!` macro. Pulse 3.0 is English-only by decision, which removed the
per-language table files, the active-language global and the plural handling
along with them.

### Contract / non-obvious behavior

- **Static vs templated.** Plain labels are read as `tr().key.clone()` — the
  field is `SharedString::new_static`, so it is the `Borrowed(&'static str)`
  variant and the clone is allocation-free (safe on the render hot path). Strings
  that interpolate runtime values are stored as `*_t` fields holding `{}`
  placeholders and are read through the methods on `impl Strings`
  (`disc`, `n_tracks`, `size`, `bp_*`, `failed_*`, `library_folder_unavailable`,
  `tags_save_failed`); those allocate a `String`,
  so call them off the hot path / cache the result.
- **Completeness is enforced by the compiler.** `lang!` expands to a full
  `Strings { .. }` struct literal, so a missing or misspelled key fails to
  compile. Adding a new string = add a field here + a line in `en.rs`.
- **No plural rules** are needed with one language: `n_tracks` is two-form
  (n == 1 vs. otherwise) and nothing else inflects.
- **`reveal_folder`** is a plain `"Open folder"` label; the app is Linux-only, so
  the old per-OS `(mac, win, other)` tuple and its `cfg` selection are gone.

## The sleep-timer table

`i18n/tools.rs` — `ToolsStrings`, a second table separate from the main one, so
the sleep timer's ~30 labels live together instead of swelling `Strings`. The
name is history: it was written for the Tools screen, which is gone, and the
timer moved into Settings → General → Sleep timer. The strings did not move with
it, so `pulse::sleep_timer::{mod,controls,settings}` still read them through
`tools_strings()`. It is live — do not treat it as dead weight. It is not
compiler-checked against `Strings`, so a key exists only if something calls it.
