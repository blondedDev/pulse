//! One-time moves of on-disk state, run before anything reads it.
//!
//! Two of them:
//!
//! - **Pre-rename directories.** Pulse 3.0 renamed the on-disk identity: the
//!   config, data and cache folders went from `pawse` to `pulse-music`
//!   (`~/.config/pulse` belongs to PulseAudio, so the plain name was not
//!   available). Existing installs would otherwise start empty — losing the
//!   settings file and, more importantly, the scanned library database.
//! - **Into a portable install.** 5.9 runs from an AppImage and keeps its data
//!   beside the image. A user upgrading from a 5.8 package has their library in
//!   `~/.local/share/pulse-music`, and a first run that ignores it looks exactly
//!   like a fresh install.
//!
//! Guardrails, in order of importance:
//! - **Never overwrite.** A move happens only when the destination is absent.
//! - **Never delete.** If a move fails, the source is left exactly as it was.
//! - **Idempotent.** Running it again after a successful move is a no-op.
//!
//! The portable migration copies rather than moves, for the same reason: the
//! old directories stay exactly as they were, so a migration that half-fails
//! cannot cost anyone their library.

use std::path::{Path, PathBuf};

/// Legacy directory name and its replacement, applied under each base folder.
const RENAMES: &[(&str, &str)] = &[("pawse", "pulse-music")];

/// Move the legacy directories if they are still around. Called once, at the
/// very start of `main`, before anything reads settings or the library.
pub fn migrate_legacy_dirs() {
    for (old, new) in RENAMES {
        for base in [dirs::config_dir(), dirs::data_dir(), dirs::cache_dir()]
            .into_iter()
            .flatten()
        {
            migrate_one(&base, old, new);
        }
    }
}

fn migrate_one(base: &Path, old: &str, new: &str) {
    let from = base.join(old);
    let to = base.join(new);
    if !from.is_dir() || to.exists() {
        return;
    }
    match std::fs::rename(&from, &to) {
        Ok(()) => log::info!("migrated {} -> {}", from.display(), to.display()),
        Err(err) => log::warn!(
            "could not migrate {} to {}: {err}; leaving it in place",
            from.display(),
            to.display()
        ),
    }
}

/// Copy a platform install's settings and library into a portable one.
///
/// Called on the first start of an AppImage, when the `data/` folder beside it
/// holds neither file yet. Copies, never moves — see the module doc.
pub fn migrate_into_portable(root: &Path) {
    let sources: Vec<PathBuf> = [dirs::config_dir(), dirs::data_dir()]
        .into_iter()
        .flatten()
        .map(|base| base.join("pulse-music"))
        .collect();
    copy_into_portable(root, &sources);
}

/// The part worth testing: explicit sources, explicit destination.
fn copy_into_portable(root: &Path, sources: &[PathBuf]) {
    /// Both live in the platform layout under their own base directory, and
    /// both are the state a fresh-looking first run would silently lose.
    const FILES: &[&str] = &["settings.json", "library.db"];

    if FILES.iter().any(|file| root.join(file).exists()) {
        return;
    }

    for source in sources {
        for file in FILES {
            let from = source.join(file);
            if !from.is_file() {
                continue;
            }
            match std::fs::copy(&from, root.join(file)) {
                Ok(_) => log::info!(
                    "portable install: copied {} into {}",
                    from.display(),
                    root.display()
                ),
                Err(err) => log::warn!(
                    "portable install: could not copy {}: {err}; the platform copy \
                     is still in place",
                    from.display()
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp_base(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pulse-migration-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_a_legacy_dir_into_place() {
        let base = tmp_base("moves");
        let src = base.join("pawse");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("settings.json"), "{}").unwrap();

        migrate_one(&base, "pawse", "pulse-music");

        assert!(!src.exists(), "legacy dir should be gone");
        assert!(
            base.join("pulse-music/settings.json").is_file(),
            "settings should have moved across"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn never_clobbers_an_existing_destination() {
        let base = tmp_base("clobber");
        let src = base.join("pawse");
        let dst = base.join("pulse-music");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("settings.json"), "old").unwrap();
        std::fs::write(dst.join("settings.json"), "new").unwrap();

        migrate_one(&base, "pawse", "pulse-music");

        assert_eq!(
            std::fs::read_to_string(dst.join("settings.json")).unwrap(),
            "new",
            "destination must not be overwritten"
        );
        assert!(src.exists(), "source must be left alone");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn is_a_noop_when_nothing_to_migrate() {
        let base = tmp_base("noop");
        migrate_one(&base, "pawse", "pulse-music");
        assert!(!base.join("pulse-music").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Upgrading from the 5.8 package must not look like a fresh install: the
    /// library and the settings come across into the portable folder.
    #[test]
    fn a_platform_install_is_copied_into_the_portable_folder() {
        let base = tmp_base("portable");
        let config = base.join("config/pulse-music");
        let data = base.join("data/pulse-music");
        let portable = base.join("Applications/Pulse/data");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&portable).unwrap();
        std::fs::write(config.join("settings.json"), "{\"theme\":\"Nord\"}").unwrap();
        std::fs::write(data.join("library.db"), "sqlite").unwrap();

        copy_into_portable(&portable, &[config.clone(), data.clone()]);

        assert_eq!(
            std::fs::read_to_string(portable.join("settings.json")).unwrap(),
            "{\"theme\":\"Nord\"}"
        );
        assert_eq!(
            std::fs::read_to_string(portable.join("library.db")).unwrap(),
            "sqlite"
        );
        assert!(
            config.join("settings.json").is_file() && data.join("library.db").is_file(),
            "the platform copies must be left in place"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Once the portable folder has its own state, the platform layout is no
    /// longer consulted — otherwise a stale copy would overwrite live data.
    #[test]
    fn a_portable_folder_that_already_has_state_is_left_alone() {
        let base = tmp_base("portable-existing");
        let data = base.join("data/pulse-music");
        let portable = base.join("Applications/Pulse/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&portable).unwrap();
        std::fs::write(data.join("library.db"), "old library").unwrap();
        std::fs::write(portable.join("library.db"), "current library").unwrap();

        copy_into_portable(&portable, &[data]);

        assert_eq!(
            std::fs::read_to_string(portable.join("library.db")).unwrap(),
            "current library"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
