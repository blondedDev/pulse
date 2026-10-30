//! The QR code for the remote-control address.
//!
//! Kept apart from the settings view because it is the one thing on that screen
//! that has to produce a file: GPUI paints images from a path, so the address is
//! encoded into a PNG under the cache directory. The file is named after a hash
//! of the address, so a steady address reuses one file instead of rewriting it
//! on every frame, while a changed address simply lands on a new name. The
//! directory is kept to that single file — anything else in it is a leftover.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::paths;

/// Pixels per QR module. A URL of this length needs 25-33 modules a side, so 6
/// gives roughly 200 px: sharp for a phone camera, small enough to sit in a
/// settings row.
const SCALE: u32 = 6;

/// Quiet zone in modules, which the spec asks for so scanners can find the
/// code against a background.
const QUIET: u32 = 4;

/// Dark modules. Not pure black: a QR on a dark UI looks less like a hole.
const INK: image::Rgba<u8> = image::Rgba([17, 17, 17, 255]);
const PAPER: image::Rgba<u8> = image::Rgba([255, 255, 255, 255]);

/// A PNG of `url` as a QR code, or `None` when it cannot be encoded or written.
pub fn path_for(url: &str) -> Option<PathBuf> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let dir = paths::cache_dir().join("remote-qr");
    let path = dir.join(format!("{:016x}.png", hasher.finish()));
    if path.exists() {
        return Some(path);
    }

    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    render(&code).save(&path).ok()?;
    drop_stale(&dir, &path);
    Some(path)
}

fn render(code: &qrcode::QrCode) -> image::RgbaImage {
    let modules = code.width() as u32;
    let side = (modules + QUIET * 2) * SCALE;
    let mut image = image::RgbaImage::from_pixel(side, side, PAPER);
    for (index, color) in code.to_colors().into_iter().enumerate() {
        if color != qrcode::Color::Dark {
            continue;
        }
        let index = index as u32;
        let left = (index % modules + QUIET) * SCALE;
        let top = (index / modules + QUIET) * SCALE;
        for y in top..top + SCALE {
            for x in left..left + SCALE {
                image.put_pixel(x, y, INK);
            }
        }
    }
    image
}

/// Remove codes for addresses that are no longer current. Best effort: a file
/// that cannot be deleted is a wasted kilobyte, not an error worth surfacing.
fn drop_stale(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path() != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test on purpose: the cache directory is shared, and a test that
    /// writes a second code deletes the first one's file.
    #[test]
    fn codes_are_cached_per_address_and_stay_square() {
        let first = path_for("http://127.0.0.1:8771/").expect("a code");
        let again = path_for("http://127.0.0.1:8771/").expect("a code");
        assert_eq!(first, again, "the same address reuses one file");
        assert!(first.exists());

        let image = image::open(&first).expect("a readable png");
        assert_eq!(image.width(), image.height());
        // 21 modules for a short URL, plus the quiet zone, times the scale.
        assert!(image.width() >= (21 + QUIET * 2) * SCALE);

        let second = path_for("http://127.0.0.1:8772/?token=abc").expect("a code");
        assert_ne!(first, second);
        assert!(!first.exists(), "the previous code should not linger");
        assert!(second.exists());
    }
}
