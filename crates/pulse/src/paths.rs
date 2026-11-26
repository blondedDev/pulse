//! Where Pulse keeps its files.
//!
//! Three cases, in this order:
//!
//! 1. `PULSE_HOME` is set — everything lives under it. That is what an exported
//!    portable copy uses, and what the screenshot harness uses.
//! 2. Pulse is running from an AppImage — everything lives in `data/` beside the
//!    image. One folder holds the player and the library it built, which is the
//!    point: an AppImage is the install, so nothing may be scattered into
//!    `~/.config` and `~/.local/share` where nobody would think to look when
//!    they delete it.
//! 3. Otherwise — the platform's config and data dirs, exactly as before.
//!
//! The AppImage case is also what makes the in-app updater work without root:
//! the image sits in a directory the user owns, so the updater can rename a
//! verified download over it.

use std::path::{Path, PathBuf};

/// The directory `PULSE_HOME` points at, if it is set.
fn env_root() -> Option<PathBuf> {
    std::env::var_os("PULSE_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// `data/` beside the AppImage, when Pulse is running from one.
///
/// `None` outside an AppImage, and also when the image sits somewhere the user
/// cannot write — an AppImage in `/opt` or on a read-only mount still has to
/// run, and silently failing to save settings is worse than falling back to the
/// platform layout.
pub fn portable_root() -> Option<PathBuf> {
    let appimage = std::env::var_os("APPIMAGE")?;
    portable_root_for(Path::new(&appimage))
}

/// The active root: `PULSE_HOME` if it is set, otherwise the AppImage's
/// `data/` folder, otherwise nothing and the platform layout applies.
fn override_root() -> Option<PathBuf> {
    env_root().or_else(portable_root)
}

/// Settings, and anything else that is configuration.
pub fn config_dir() -> Option<PathBuf> {
    override_root().or_else(|| dirs::config_dir().map(|base| base.join("pulse-music")))
}

/// The library database, and anything else that is data.
pub fn data_dir() -> Option<PathBuf> {
    override_root().or_else(|| dirs::data_dir().map(|base| base.join("pulse-music")))
}

/// Cache. Never part of an export, but it follows `PULSE_HOME` so a portable
/// copy does not write into the host's cache either.
pub fn cache_dir() -> PathBuf {
    override_root().unwrap_or_else(|| {
        dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("pulse-music")
    })
}

/// The portable root for a given AppImage path, created if it is missing.
///
/// Split out from [`portable_root`] so the behaviour can be tested without
/// setting a process-global environment variable.
fn portable_root_for(appimage: &Path) -> Option<PathBuf> {
    let root = appimage.parent()?.join("data");
    std::fs::create_dir_all(&root).ok()?;
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulse-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The whole point of the portable layout: the folder that holds the
    /// AppImage also holds the data, so deleting the folder uninstalls Pulse.
    #[test]
    fn an_appimage_keeps_its_data_beside_itself() {
        let dir = scratch("beside");
        let appimage = dir.join("Pulse.AppImage");
        std::fs::write(&appimage, b"not really an appimage").expect("write");

        let root = portable_root_for(&appimage).expect("a writable folder");
        assert_eq!(root, dir.join("data"));
        assert!(root.is_dir(), "the data folder is created for the first run");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An AppImage can live anywhere, including somewhere unwritable. Failing
    /// to save settings silently is worse than using the platform layout.
    #[test]
    fn a_read_only_folder_falls_back_instead_of_failing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("readonly");
        let appimage = dir.join("Pulse.AppImage");
        std::fs::write(&appimage, b"x").expect("write");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        let root = portable_root_for(&appimage);

        // Restore before asserting: a failing assert must not leave a directory
        // the test run cannot clean up.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(root, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Setting an environment variable is process-global, so these assert the
    /// pieces that do not depend on it and leave the variable alone.
    #[test]
    fn without_an_override_the_platform_layout_is_used() {
        // The test process does not set PULSE_HOME; if a future test does, this
        // is the one that will explain why other things broke.
        if std::env::var_os("PULSE_HOME").is_none() && std::env::var_os("APPIMAGE").is_none() {
            assert!(
                config_dir()
                    .map(|dir| dir.ends_with("pulse-music"))
                    .unwrap_or(true)
            );
            assert!(
                data_dir()
                    .map(|dir| dir.ends_with("pulse-music"))
                    .unwrap_or(true)
            );
        }
    }
}
