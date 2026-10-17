//! Exporting a copy of Pulse that carries its own data.
//!
//! The result is a folder you can copy anywhere: the binary, the files you
//! ticked, and a launcher that sets `PULSE_HOME` so the copy reads *its own*
//! settings and library instead of the ones it was exported from.

use std::path::{Path, PathBuf};

/// What to put in the export besides the program itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub settings: bool,
    pub library: bool,
}

/// The folder name inside the destination.
const FOLDER: &str = "Pulse";

/// Copy the program and the chosen files into `destination`.
///
/// Returns the folder it created. Nothing in the source is modified — every
/// step here is a read or a copy.
pub fn export(destination: &Path, options: ExportOptions) -> anyhow::Result<PathBuf> {
    let root = destination.join(FOLDER);
    let data = root.join("data");
    std::fs::create_dir_all(&data)?;

    let exe = std::env::current_exe()?;
    let program = root.join("pulse");
    std::fs::copy(&exe, &program)?;
    make_executable(&program)?;

    if options.settings
        && let Some(path) = crate::paths::config_dir().map(|dir| dir.join("settings.json"))
        && path.is_file()
    {
        std::fs::copy(&path, data.join("settings.json"))?;
    }

    if options.library
        && let Some(path) = crate::paths::data_dir().map(|dir| dir.join("library.db"))
        && path.is_file()
    {
        // `VACUUM INTO` is the safe way to copy a live SQLite database: it writes
        // a consistent snapshot instead of whatever bytes happen to be on disk
        // mid-transaction.
        copy_database(&path, &data.join("library.db"))?;
    }

    let launcher = root.join("Pulse.sh");
    std::fs::write(
        &launcher,
        "#!/bin/sh\n\
         # Runs this copy of Pulse with its own data directory.\n\
         here=$(cd \"$(dirname \"$0\")\" && pwd)\n\
         PULSE_HOME=\"$here/data\" exec \"$here/pulse\" \"$@\"\n",
    )?;
    make_executable(&launcher)?;

    Ok(root)
}

/// A consistent snapshot of a possibly-live database.
fn copy_database(source: &Path, target: &Path) -> anyhow::Result<()> {
    let conn = rusqlite::Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // `VACUUM INTO` refuses to overwrite an existing file, which is what we want
    // here: the destination is a folder we just created.
    let escaped = target.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_contains_the_program_a_launcher_and_only_what_was_asked_for() {
        let dir = std::env::temp_dir().join(format!("pulse-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let root = export(
            &dir,
            ExportOptions {
                settings: false,
                library: false,
            },
        )
        .unwrap();

        assert!(root.join("pulse").is_file(), "the program must be there");
        assert!(root.join("Pulse.sh").is_file(), "so must the launcher");

        let launcher = std::fs::read_to_string(root.join("Pulse.sh")).unwrap();
        assert!(
            launcher.contains("PULSE_HOME=\"$here/data\""),
            "the launcher is what makes the copy independent: {launcher}"
        );

        // Barebones: no data folder contents.
        assert!(!root.join("data/settings.json").exists());
        assert!(!root.join("data/library.db").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
