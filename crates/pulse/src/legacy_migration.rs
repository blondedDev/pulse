//! One-time move of the pre-rename (Pawse 2.x) directories to their Pulse names.
//!
//! Pulse 3.0 renamed the on-disk identity: the config, data and cache folders
//! went from `pawse` to `pulse-music` (`~/.config/pulse` belongs to PulseAudio,
//! so the plain name was not available). Existing installs would otherwise
//! start empty — losing the settings file and, more importantly, the scanned
//! library database.
//!
//! Guardrails, in order of importance:
//! - **Never overwrite.** A move happens only when the destination is absent.
//! - **Never delete.** If a move fails, the source is left exactly as it was.
//! - **Idempotent.** Running it again after a successful move is a no-op.

use std::path::Path;

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
}
