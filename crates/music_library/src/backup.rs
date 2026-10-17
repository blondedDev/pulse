//! Rotated database backups.
//!
//! The one thing in this crate that protects the user's data from everything
//! else in this crate. A backup is taken with `VACUUM INTO`, which is the only
//! safe way to copy a live WAL database — a plain file copy of a database with
//! a populated `-wal` can produce a file that opens but is missing the newest
//! transactions, or does not open at all.
//!
//! Two rules the rest of this module exists to enforce:
//!
//! 1. **Atomic.** The copy lands in `<name>.partial` and is renamed into place,
//!    so a crash or a full disk cannot leave a truncated file wearing a name
//!    that looks like a good backup.
//! 2. **Bounded.** Backups are pruned to a daily window plus one per month, so
//!    a long-lived install cannot fill the disk with copies of the same
//!    library. See [`backups_to_delete`].
//!
//! Backup names carry the database's file stem (`library-20261016.db`). That
//! makes them self-describing, and it keeps two databases that happen to share
//! a directory from pruning each other's history — which is what a test suite
//! full of throwaway databases would otherwise do.

use std::path::{Path, PathBuf};

/// How many recent daily backups to keep before pruning starts.
///
/// Chosen so a week of "I broke something yesterday" is recoverable, while the
/// monthly rule covers the "I broke something last month" case.
pub const DEFAULT_KEEP_DAILY_BACKUPS: usize = 7;

/// Suffix of a backup file name.
const SUFFIX: &str = ".db";

/// Days since the Unix epoch for a Unix timestamp.
///
/// Integer division, so this is the UTC day the timestamp falls in. Local time
/// would be friendlier in the file name, but the *decision* this feeds ("has a
/// backup been taken today?") only has to be self-consistent, and staying in
/// UTC keeps the function pure and the tests deterministic.
fn days_since_epoch(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(86_400)
}

/// Civil date (year, month, day) from days since the Unix epoch.
///
/// Howard Hinnant's `civil_from_days`: shift the epoch to 0000-03-01 so leap
/// days land at the end of the year, then recover the era, the year-of-era and
/// the day-of-year with integer arithmetic. No lookup tables, no dependency on
/// a date crate, and the final branch is the leap-year adjustment.
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

/// The file name a backup of `stem` taken at `unix_secs` should have.
pub fn backup_file_name(stem: &str, unix_secs: i64) -> String {
    let (y, m, d) = civil_from_days(days_since_epoch(unix_secs));
    format!("{stem}-{y:04}{m:02}{d:02}{SUFFIX}")
}

/// True when `name` is a backup of `stem` that this module wrote.
///
/// Deliberately strict: exactly `<stem>-`, eight digits, `.db`. Anything else in
/// the directory (a `.partial`, a hand-made copy, a backup belonging to a
/// different database) is left alone by the pruner.
pub fn is_backup_name(stem: &str, name: &str) -> bool {
    let Some(digits) = name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_suffix(SUFFIX))
    else {
        return false;
    };
    digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Which of `names` to delete, given the names present and a daily window.
///
/// Keeps the newest `retain` outright, then keeps the newest backup of each
/// month from what is left, and returns everything else. The second rule is what
/// makes "the library broke last month" survivable without keeping 365 files.
///
/// Order does not matter on the way in; names are compared as strings, which is
/// chronological because the date is zero-padded and big-endian.
pub fn backups_to_delete<S: AsRef<str>>(stem: &str, names: &[S], retain: usize) -> Vec<String> {
    let mut backups: Vec<&str> = names
        .iter()
        .map(AsRef::as_ref)
        .filter(|name| is_backup_name(stem, name))
        .collect();
    // Newest first.
    backups.sort_unstable_by(|a, b| b.cmp(a));

    let date_start = stem.len() + 1;
    let mut doomed = Vec::new();
    let mut months_kept: Vec<&str> = Vec::new();
    for (ix, name) in backups.iter().enumerate() {
        if ix < retain {
            continue;
        }
        // Within the monthly window, one survivor per `YYYYMM`.
        let month = &name[date_start..date_start + 6];
        if months_kept.contains(&month) {
            doomed.push((*name).to_string());
        } else {
            months_kept.push(month);
        }
    }
    doomed
}

/// Where backups for a database live, and what they are called.
///
/// A sibling `backups/` directory, with the database's file stem as the name
/// prefix. Returns `None` for a path with no file name, which cannot happen for
/// a real database and does not deserve an error type of its own.
pub fn backup_location(db_path: &Path) -> Option<(PathBuf, String)> {
    let stem = db_path.file_stem()?.to_string_lossy().into_owned();
    Some((db_path.parent()?.join("backups"), stem))
}

/// One rotated backup on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupEntry {
    pub name: String,
    pub path: PathBuf,
    /// Size on disk, for showing "how much would I lose" honestly.
    pub bytes: u64,
}

/// Every backup for this database, newest first.
///
/// Ordered by name, which is chronological because the date is zero-padded and
/// big-endian — no need to parse anything.
pub fn list_backups(db_path: &Path) -> Vec<BackupEntry> {
    let Some((dir, stem)) = backup_location(db_path) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<BackupEntry> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !is_backup_name(&stem, &name) {
                return None;
            }
            let path = dir.join(&name);
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            Some(BackupEntry { name, path, bytes })
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        // Epoch, a leap day, a month boundary, a century non-leap year. The day
        // numbers are also asserted through the epoch seconds used elsewhere.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_742), (2026, 10, 16));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 1900 was not a leap year, and the day after 1900-01-01 is the 2nd.
        assert_eq!(civil_from_days(-25_567), (1900, 1, 1));
        assert_eq!(civil_from_days(-25_566), (1900, 1, 2));
        // Negative days round toward negative infinity, not toward zero.
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn backup_names_carry_the_stem_and_sort_chronologically() {
        assert_eq!(backup_file_name("library", 0), "library-19700101.db");
        assert_eq!(
            backup_file_name("library", 1_792_108_800),
            "library-20261016.db"
        );
        assert_eq!(
            backup_file_name("test-12-3", 1_792_108_800),
            "test-12-3-20261016.db"
        );

        let mut names = vec![
            backup_file_name("library", 0),
            backup_file_name("library", 1_792_108_800),
            backup_file_name("library", 1_792_108_800 - 86_400),
        ];
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "library-19700101.db".to_string(),
                "library-20261015.db".to_string(),
                "library-20261016.db".to_string(),
            ]
        );
    }

    #[test]
    fn only_our_own_names_are_recognised() {
        assert!(is_backup_name("library", "library-20261016.db"));
        assert!(!is_backup_name("library", "library-20261016.db.partial"));
        assert!(!is_backup_name("library", "library-replaced-20261016.db"));
        assert!(!is_backup_name("library", "library-2026101.db"));
        assert!(!is_backup_name("library", "library-2026101a.db"));
        assert!(!is_backup_name("library", "library.db"));
        assert!(!is_backup_name("library", "other-20261016.db"));
        // A backup of a *different* database is not ours, even though the date
        // part looks right. This is the collision the stem prefix prevents.
        assert!(!is_backup_name(
            "test-12-3",
            "library-20261016.db"
        ));
        assert!(is_backup_name(
            "test-12-3",
            "test-12-3-20261016.db"
        ));
    }

    fn days_from(start_day: i64, count: usize) -> Vec<String> {
        (0..count)
            .map(|i| backup_file_name("library", (start_day - i as i64) * 86_400))
            .collect()
    }

    #[test]
    fn a_window_below_the_retain_count_deletes_nothing() {
        let names = days_from(20_000, 5);
        assert!(backups_to_delete("library", &names, 7).is_empty());
    }

    #[test]
    fn the_window_is_kept_and_one_extra_survives_per_month() {
        // 10 consecutive days in the same month: keep 3, then the monthly rule
        // rescues exactly one more.
        let names = days_from(20_009, 10);
        let doomed = backups_to_delete("library", &names, 3);
        for keep in &names[..3] {
            assert!(!doomed.contains(keep), "{keep} must survive");
        }
        assert!(!doomed.is_empty(), "older days must be pruned");
        assert_eq!(names.len() - doomed.len(), 4);
        // The single extra survivor is the newest backup of the month that the
        // window did not already cover.
        assert!(!doomed.contains(&names[3]));
    }

    #[test]
    fn one_backup_per_month_survives_beyond_the_window() {
        // 40 days, keep 5: the window covers ~5 days, so the monthly rule must
        // rescue the newest backup of each earlier month on its own merits.
        let mut names = days_from(20_000, 40);
        names.push("library-20050930.db".to_string());
        names.push("library-20050831.db".to_string());
        let doomed = backups_to_delete("library", &names, 5);
        assert!(!doomed.contains(&"library-20050930.db".to_string()));
        assert!(!doomed.contains(&"library-20050831.db".to_string()));
    }

    #[test]
    fn pruning_ignores_files_that_are_not_ours() {
        let names = vec![
            "library-20261016.db".to_string(),
            "library-20261016.db.partial".to_string(),
            "library-replaced-20261016.db".to_string(),
            "other-20261016.db".to_string(),
            "notes.txt".to_string(),
        ];
        assert!(backups_to_delete("library", &names, 1).is_empty());
    }

    #[test]
    fn listing_backups_returns_only_ours_newest_first() {
        let dir = std::env::temp_dir().join(format!("pulse-backup-list-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("backups")).unwrap();
        let db = dir.join("library.db");
        let backups = dir.join("backups");

        for name in [
            "library-20260101.db",
            "library-20260301.db",
            "library-20260201.db",
            // Not ours: a partial, another database, and a discarded copy.
            "library-20260401.db.partial",
            "other-20260401.db",
            "library-replaced-20260401.db",
        ] {
            std::fs::write(backups.join(name), b"x").unwrap();
        }

        let listed: Vec<String> = list_backups(&db).into_iter().map(|b| b.name).collect();
        assert_eq!(
            listed,
            vec![
                "library-20260301.db",
                "library-20260201.db",
                "library-20260101.db"
            ],
            "newest first, and nothing that is not a backup of this database"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_location_sits_beside_the_database() {
        let (dir, stem) = backup_location(Path::new("/data/pulse-music/library.db")).unwrap();
        assert_eq!(dir, Path::new("/data/pulse-music/backups"));
        assert_eq!(stem, "library");

        let (dir, stem) = backup_location(Path::new("/tmp/x/test-7-2.db")).unwrap();
        assert_eq!(dir, Path::new("/tmp/x/backups"));
        assert_eq!(stem, "test-7-2");
    }
}
