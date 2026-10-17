//! Where Pulse keeps its files.
//!
//! One resolver, so an exported copy can be told to use its own data. Set
//! `PULSE_HOME` and everything — settings, database, artwork cache — lives under
//! that directory instead of the platform's config and data dirs. Unset (the
//! normal case), the behaviour is exactly what it was before.

use std::path::PathBuf;

/// The directory `PULSE_HOME` points at, if it is set.
fn override_root() -> Option<PathBuf> {
    std::env::var_os("PULSE_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Setting an environment variable is process-global, so these assert the
    /// pieces that do not depend on it and leave the variable alone.
    #[test]
    fn without_an_override_the_platform_layout_is_used() {
        // The test process does not set PULSE_HOME; if a future test does, this
        // is the one that will explain why other things broke.
        if std::env::var_os("PULSE_HOME").is_none() {
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
