//! Self-update from GitHub Releases.
//!
//! The check is **manual by design**: Pulse is offline-first, and nothing here
//! touches the network until the user presses Check for updates. Once a newer
//! tag exists, the matching asset is downloaded and checked against the
//! release's own `.sha256`, then put in place.
//!
//! There are two shapes to replace:
//!
//! - a bare binary from the release tarball, renamed over the running
//!   executable;
//! - an AppImage, where the running executable is inside a read-only mount.
//!   The AppImage runtime exports `APPIMAGE`, the path of the file the user
//!   actually launched, and that file is what gets replaced.
//!
//! Linux keeps the old inode alive for the current process, so the swap is safe
//! while the app is still painting; then the new binary is spawned and the old
//! one quits.
//!
//! Outside an AppImage the target is `std::env::current_exe()`, not a guessed
//! install path. Pulse ships as a bare binary in `~/.local/bin`,
//! `~/Applications`, or a distro package, and only the process knows which one
//! it was started from.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, Context, SharedString};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_URL: &str = "https://api.github.com/repos/blondedDev/pulse/releases/latest";
const USER_AGENT: &str = concat!("Pulse/", env!("CARGO_PKG_VERSION"), " (Linux)");
/// The tarball is 28 MB today; the cap only exists so a wrong URL cannot stream
/// forever.
const MAX_ARCHIVE_BYTES: u64 = 200 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// What the Updates row is showing, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// Nothing has been asked yet.
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists. Nothing is downloaded until the user asks:
    /// the release notes are here to be read first.
    Available { release: Release },
    /// A newer release exists and is on its way down.
    Downloading { version: String },
    /// The new binary is running; this process is about to quit.
    Restarting { version: String },
    Failed(String),
}

impl UpdateState {
    /// True while a check or install is running, so the button can be disabled.
    pub fn busy(&self) -> bool {
        matches!(
            self,
            UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Restarting { .. }
        )
    }
}

/// A release body, reduced to the lines worth showing in a settings row.
///
/// GitHub bodies are markdown with headers, badges, install blocks and
/// horizontal rules. The updater shows the plain lines so a user can see what
/// changed before installing; links, images and fences are dropped because
/// there is nothing in a settings row to open them with.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Notes {
    lines: Vec<SharedString>,
}

impl Notes {
    /// Lines kept. Enough for a release's headline points, short enough that
    /// the settings page does not turn into a changelog reader.
    const MAX_LINES: usize = 10;

    fn parse(body: &str) -> Self {
        let mut lines = Vec::new();
        let mut in_fence = false;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            // The release body carries install instructions after a rule; the
            // notes end there.
            if trimmed == "---" {
                break;
            }
            if in_fence || trimmed.is_empty() {
                continue;
            }
            // Images and badges carry no text worth showing; links keep their
            // label.
            if trimmed.starts_with("![") || trimmed.starts_with("<!--") {
                continue;
            }
            let text = strip_links(trimmed.trim_start_matches('#').trim());
            let text = text.trim_start_matches(['-', '*']).trim();
            if text.is_empty() {
                continue;
            }
            lines.push(SharedString::from(text.to_string()));
            if lines.len() == Self::MAX_LINES {
                break;
            }
        }
        Self { lines }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn lines(&self) -> &[SharedString] {
        &self.lines
    }
}

/// `[label](url)` becomes `label`; anything else is left alone.
fn strip_links(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let (before, from_start) = rest.split_at(start);
        out.push_str(before);
        let Some(mid) = from_start.find("](") else {
            out.push_str(from_start);
            return out;
        };
        let label = &from_start[1..mid];
        let after = &from_start[mid + 2..];
        match after.find(')') {
            Some(end) => {
                out.push_str(label);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(from_start);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The updater. One per process, held by `Services`; the Settings row reads it
/// and the click handler drives it.
pub struct Updater {
    state: UpdateState,
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub fn new() -> Self {
        Self {
            state: UpdateState::Idle,
        }
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Check, download, install, restart. At most one run at a time.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.state.busy() {
            return;
        }
        // Releases are built for x86_64 Linux only; pretending otherwise would
        // install a binary this machine cannot run.
        if std::env::consts::ARCH != "x86_64" {
            self.state = UpdateState::Failed(
                "Updates are only published for x86_64 Linux.".to_string(),
            );
            cx.notify();
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let latest = cx
                .background_executor()
                .spawn(async { fetch_latest() })
                .await;
            let latest = match latest {
                Ok(latest) => latest,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.state = UpdateState::Failed(error);
                        cx.notify();
                    });
                    return;
                }
            };

            if !is_newer(&latest.version, env!("CARGO_PKG_VERSION")) {
                let _ = this.update(cx, |this, cx| {
                    this.state = UpdateState::UpToDate;
                    cx.notify();
                });
                return;
            }

            let _ = this.update(cx, |this, cx| {
                this.state = UpdateState::Available { release: latest };
                cx.notify();
            });
        })
        .detach();
    }

    /// Download and install the release the last check found.
    ///
    /// Separate from [`check`](Self::check) so the notes can be read first: an
    /// update that starts downloading the moment it is found gives nobody a
    /// chance to see what changed.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available { release } = self.state.clone() else {
            return;
        };
        let version = release.version.clone();
        self.state = UpdateState::Downloading {
            version: version.clone(),
        };
        cx.notify();

        cx.spawn(async move |this, cx| {
            let download = release.clone();
            let installed = cx
                .background_executor()
                .spawn(async move { download_and_install(&download) })
                .await;

            match installed {
                Ok(()) => {
                    let restarted = this
                        .update(cx, |this, cx| {
                            this.state = UpdateState::Restarting {
                                version: version.clone(),
                            };
                            cx.notify();
                            restart(cx)
                        })
                        .unwrap_or(false);
                    if !restarted {
                        let _ = this.update(cx, |this, cx| {
                            this.state = UpdateState::Failed(
                                "The new version was installed but could not be started. \
                                 Reopen Pulse to use it."
                                    .to_string(),
                            );
                            cx.notify();
                        });
                    }
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.state = UpdateState::Failed(error);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

fn restart(cx: &mut App) -> bool {
    let Ok(exe) = restarted_exe() else {
        return false;
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if std::process::Command::new(&exe).args(args).spawn().is_ok() {
        cx.quit();
        true
    } else {
        false
    }
}

/// The file to start after an update: the AppImage when there is one, the
/// executable otherwise.
fn restarted_exe() -> Result<PathBuf, std::io::Error> {
    match running_appimage() {
        Some(appimage) => Ok(appimage),
        None => std::env::current_exe(),
    }
}

/// The AppImage this process was started from, if any.
///
/// The AppImage runtime sets `APPIMAGE` to the file the user launched. Inside
/// the mounted image `current_exe()` points at the read-only extract, which is
/// why the updater cannot simply rename over it.
fn running_appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Asset {
    url: String,
    checksum_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The release body, already split into readable lines.
    pub notes: Notes,
    /// The tarball with the bare binary. Always present.
    archive: Asset,
    /// Published alongside the tarball; optional so an older release still
    /// updates a bare binary, but required when *this* copy is an AppImage.
    appimage: Option<Asset>,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Deserialize, Clone)]
struct AssetJson {
    name: String,
    browser_download_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            // The tarball is 28 MB; a slow link should still finish.
            .timeout_recv_body(Some(Duration::from_secs(300)))
            .build(),
    )
}

fn fetch_latest() -> Result<Release, String> {
    let agent = agent();
    let mut response = agent
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("Could not reach GitHub: {error}"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(match status {
            403 | 429 => "GitHub is rate-limiting update checks. Try again later.".to_string(),
            _ => format!("GitHub answered with HTTP {status}."),
        });
    }
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_TEXT_BYTES)
        .read_to_string()
        .map_err(|error| format!("Could not read GitHub's answer: {error}"))?;
    let json: ReleaseJson =
        serde_json::from_str(&text).map_err(|error| format!("Unexpected release data: {error}"))?;
    release_from_json(json)
}

fn release_from_json(json: ReleaseJson) -> Result<Release, String> {
    let archive = pick_asset(
        &json,
        "-x86_64-linux.tar.gz",
        "The latest release has no Linux archive.",
    )?;
    // Missing is not an error: a bare install still has the tarball to use.
    let appimage = pick_asset(
        &json,
        "-x86_64.AppImage",
        "The latest release has no AppImage.",
    )
    .ok();

    Ok(Release {
        version: json.tag_name,
        notes: Notes::parse(&json.body),
        archive,
        appimage,
    })
}

/// Finds an asset by suffix, with its `.sha256` beside it.
fn pick_asset(json: &ReleaseJson, suffix: &str, missing: &str) -> Result<Asset, String> {
    let asset = json
        .assets
        .iter()
        .find(|asset| asset.name.ends_with(suffix))
        .ok_or_else(|| missing.to_string())?;
    let checksum_name = format!("{}.sha256", asset.name);
    let checksum = json
        .assets
        .iter()
        .find(|asset| asset.name == checksum_name)
        .ok_or_else(|| "The latest release has no checksum file.".to_string())?;

    Ok(Asset {
        url: asset.browser_download_url.clone(),
        checksum_url: checksum.browser_download_url.clone(),
    })
}

/// Download, verify and install. Runs off the UI thread.
fn download_and_install(release: &Release) -> Result<(), String> {
    // An AppImage has to be replaced as a file; anything else installs the
    // bare binary out of the tarball. Picking the asset first means a release
    // that forgot to attach the AppImage fails before downloading 30 MB.
    let (asset, target, appimage) = match running_appimage() {
        Some(appimage) => {
            let asset = release.appimage.clone().ok_or_else(|| {
                "The latest release has no AppImage to update this copy with.".to_string()
            })?;
            (asset, appimage, true)
        }
        None => {
            let target = std::env::current_exe()
                .map_err(|error| format!("Could not find the running Pulse binary: {error}"))?;
            (release.archive.clone(), target, false)
        }
    };

    let agent = agent();
    let mut checksum = agent
        .get(&asset.checksum_url)
        .call()
        .map_err(|error| format!("Could not download the checksum: {error}"))?;
    let checksum_text = checksum
        .body_mut()
        .with_config()
        .limit(MAX_TEXT_BYTES)
        .read_to_string()
        .map_err(|error| format!("Could not read the checksum: {error}"))?;
    let expected = expected_sha(&checksum_text)
        .ok_or_else(|| "The release checksum was not a SHA-256 hash.".to_string())?;

    let mut download = agent
        .get(&asset.url)
        .call()
        .map_err(|error| format!("Could not download the update: {error}"))?;
    let bytes = download
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE_BYTES)
        .read_to_vec()
        .map_err(|error| format!("Could not read the update: {error}"))?;

    if appimage {
        install_appimage(&bytes, &expected, &target)
    } else {
        install_bytes(&bytes, &expected, &target)
    }
}

/// Verify `bytes`, take the `pulse` entry out of the tarball, and rename it
/// over `target` in one step.
///
/// The staged file lives beside the target, because `rename` is only atomic
/// within one filesystem, and the target's directory is the one a user is
/// expected to own.
fn install_bytes(bytes: &[u8], expected_sha: &str, target: &Path) -> Result<(), String> {
    verify_download(bytes, expected_sha)?;

    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("Could not read the update archive: {error}"))?;

    let staged = staged_path(target);
    let mut found = false;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("Could not read the update archive: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("Could not read the update archive: {error}"))?;
        if path.file_name().and_then(|name| name.to_str()) != Some("pulse") {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let mut file = std::fs::File::create(&staged).map_err(|error| {
            format!(
                "Could not write next to {}: {error}. Install Pulse somewhere your user owns.",
                target.display()
            )
        })?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|error| format!("Could not unpack the update: {error}"))?;
        drop(file);
        set_executable(&staged)?;
        std::fs::rename(&staged, target).map_err(|error| {
            let _ = std::fs::remove_file(&staged);
            format!("Could not replace {}: {error}", target.display())
        })?;
        found = true;
        break;
    }

    if found {
        Ok(())
    } else {
        let _ = std::fs::remove_file(&staged);
        Err("The release archive did not contain the Pulse binary.".to_string())
    }
}

fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    target.with_file_name(name)
}

/// Verify a downloaded asset against the release's `.sha256`.
fn verify_download(bytes: &[u8], expected_sha: &str) -> Result<(), String> {
    let actual = hex_digest(&Sha256::digest(bytes));
    if actual != expected_sha.to_ascii_lowercase() {
        return Err(
            "The update was corrupted (checksum mismatch). Nothing was changed.".to_string(),
        );
    }
    Ok(())
}

/// Replace the AppImage the user launched with the downloaded one.
///
/// The image is written beside it first: the mount the process is running from
/// stays valid, and the rename is atomic on the same filesystem. The file has
/// to be executable before the swap, or the next launch is a permission error.
fn install_appimage(bytes: &[u8], expected_sha: &str, target: &Path) -> Result<(), String> {
    verify_download(bytes, expected_sha)?;

    let staged = staged_path(target);
    std::fs::write(&staged, bytes).map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        format!(
            "Could not write next to {}: {error}. Move the AppImage to a folder your user can write to.",
            target.display()
        )
    })?;
    if let Err(error) = set_executable(&staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    std::fs::rename(&staged, target).map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        format!("Could not replace {}: {error}", target.display())
    })
}

fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("Could not make the update executable: {error}"))
}

fn hex_digest(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest.iter().fold(String::with_capacity(64), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The first word of a `.sha256` file, which `sha256sum` writes as
/// `hash  filename`.
fn expected_sha(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?;
    if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(hash.to_ascii_lowercase())
    } else {
        None
    }
}

/// `v5.7.0` (or `5.7`, or `5.7.0-rc1`) as a comparable triple.
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = tag.trim();
    let tag = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    // Pre-release and build metadata do not change which release is newer here:
    // this repo only publishes numeric tags.
    let core = tag.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    /// The settings row shows these lines; fenced install blocks, images and
    /// link syntax are noise there.
    #[test]
    fn release_notes_keep_the_readable_lines() {
        let body = "Prebuilt for **x86_64 Linux**.\n\n                    ![screenshot](https://example.test/a.png)\n                    ## Highlights\n                    - The AppImage is the install\n                    * Settings live beside it\n                    ```sh\n\
                    mv Pulse.AppImage ~/Applications/Pulse/\n\
                    ```\n                    See [the notes](https://example.test/notes) for more.\n\
                    ---\n\
                    Prebuilt for **x86_64 Linux**.\n";
        let notes = Notes::parse(body);
        assert_eq!(
            notes.lines(),
            [
                "Prebuilt for **x86_64 Linux**.",
                "Highlights",
                "The AppImage is the install",
                "Settings live beside it",
                "See the notes for more.",
            ]
        );
    }

    #[test]
    fn release_body_reaches_the_release() {
        let json: ReleaseJson = serde_json::from_str(
            r#"{
                "tag_name": "v9.9.9",
                "body": "- one\n- two",
                "assets": [
                    {"name": "pulse-v9.9.9-x86_64-linux.tar.gz",
                     "browser_download_url": "https://example.test/tar"},
                    {"name": "pulse-v9.9.9-x86_64-linux.tar.gz.sha256",
                     "browser_download_url": "https://example.test/sha"}
                ]
            }"#,
        )
        .unwrap();
        let release = release_from_json(json).unwrap();
        assert_eq!(release.notes.lines(), ["one", "two"]);
    }

    #[test]
    fn a_release_without_a_body_has_no_notes() {
        let json: ReleaseJson = serde_json::from_str(
            r#"{
                "tag_name": "v9.9.9",
                "assets": [
                    {"name": "pulse-v9.9.9-x86_64-linux.tar.gz",
                     "browser_download_url": "https://example.test/tar"},
                    {"name": "pulse-v9.9.9-x86_64-linux.tar.gz.sha256",
                     "browser_download_url": "https://example.test/sha"}
                ]
            }"#,
        )
        .unwrap();
        assert!(release_from_json(json).unwrap().notes.is_empty());
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v5.10.0", "5.9.9"), "10 is newer than 9");
        assert!(is_newer("5.6.2", "5.6.1"));
        assert!(!is_newer("5.6.1", "5.6.1"));
        assert!(!is_newer("v5.5.9", "5.6.1"));
        assert!(!is_newer("not-a-version", "5.6.1"));
        assert!(is_newer("5.7", "5.6.9"), "a missing patch is zero");
    }

    #[test]
    fn checksum_parsing_accepts_sha256sum_output() {
        let hash = "a".repeat(64);
        assert_eq!(expected_sha(&format!("{hash}  pulse.tar.gz\n")), Some(hash.clone()));
        assert_eq!(expected_sha(&hash.to_uppercase()), Some(hash));
        assert_eq!(expected_sha("deadbeef  pulse.tar.gz"), None);
        assert_eq!(expected_sha(""), None);
    }

    #[test]
    fn the_linux_archive_and_its_checksum_are_the_ones_picked() {
        let json: ReleaseJson = serde_json::from_str(
            r#"{
                "tag_name": "v5.7.0",
                "assets": [
                    {"name": "pulse-v5.7.0-x86_64-linux.tar.gz.sha256",
                     "browser_download_url": "https://example.test/sha"},
                    {"name": "source.zip", "browser_download_url": "https://example.test/src"},
                    {"name": "pulse-v5.7.0-x86_64-linux.tar.gz",
                     "browser_download_url": "https://example.test/tar"}
                ]
            }"#,
        )
        .unwrap();
        let release = release_from_json(json).unwrap();
        assert_eq!(release.version, "v5.7.0");
        assert_eq!(release.archive.url, "https://example.test/tar");
        assert_eq!(release.archive.checksum_url, "https://example.test/sha");
        assert!(release.appimage.is_none());
    }

    #[test]
    fn the_appimage_and_its_checksum_are_picked_when_present() {
        let json: ReleaseJson = serde_json::from_str(
            r#"{
                "tag_name": "v5.8.1",
                "assets": [
                    {"name": "Pulse-v5.8.1-x86_64.AppImage.sha256",
                     "browser_download_url": "https://example.test/appimage-sha"},
                    {"name": "pulse-v5.8.1-x86_64-linux.tar.gz.sha256",
                     "browser_download_url": "https://example.test/tar-sha"},
                    {"name": "Pulse-v5.8.1-x86_64.AppImage",
                     "browser_download_url": "https://example.test/appimage"},
                    {"name": "pulse-v5.8.1-x86_64-linux.tar.gz",
                     "browser_download_url": "https://example.test/tar"}
                ]
            }"#,
        )
        .unwrap();
        let release = release_from_json(json).unwrap();
        let appimage = release.appimage.expect("the AppImage asset is found");
        assert_eq!(appimage.url, "https://example.test/appimage");
        assert_eq!(appimage.checksum_url, "https://example.test/appimage-sha");
        assert_eq!(release.archive.url, "https://example.test/tar");
    }

    #[test]
    fn an_appimage_without_a_checksum_is_not_offered() {
        let json: ReleaseJson = serde_json::from_str(
            r#"{
                "tag_name": "v5.8.1",
                "assets": [
                    {"name": "Pulse-v5.8.1-x86_64.AppImage",
                     "browser_download_url": "https://example.test/appimage"},
                    {"name": "pulse-v5.8.1-x86_64-linux.tar.gz.sha256",
                     "browser_download_url": "https://example.test/tar-sha"},
                    {"name": "pulse-v5.8.1-x86_64-linux.tar.gz",
                     "browser_download_url": "https://example.test/tar"}
                ]
            }"#,
        )
        .unwrap();
        assert!(release_from_json(json).unwrap().appimage.is_none());
    }

    /// Opt-in network check, because it is the only way to catch GitHub changing
    /// the asset names underneath us.
    #[test]
    #[ignore = "hits the GitHub API; run with --ignored"]
    fn the_live_release_is_parsable() {
        let release = fetch_latest().unwrap();
        assert!(
            release.archive.url.ends_with("-x86_64-linux.tar.gz"),
            "{}",
            release.archive.url
        );
        assert!(release.archive.checksum_url.ends_with(".sha256"));
        if let Some(appimage) = release.appimage {
            assert!(
                appimage.url.ends_with("-x86_64.AppImage"),
                "{}",
                appimage.url
            );
            assert!(appimage.checksum_url.ends_with(".sha256"));
        }
    }

    /// Build a release-shaped archive in memory: a `pulse` file plus a licence.
    fn archive(contents: &[u8]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "./pulse", contents).unwrap();
        let mut licence = tar::Header::new_gnu();
        licence.set_size(1);
        licence.set_mode(0o644);
        licence.set_cksum();
        tar.append_data(&mut licence, "./LICENSE", &b"x"[..]).unwrap();
        let tar = tar.into_inner().unwrap();

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn staged_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pulse-updater-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_verified_archive_replaces_the_target() {
        let dir = staged_dir("install");
        let target = dir.join("pulse");
        std::fs::write(&target, b"old").unwrap();
        let bytes = archive(b"new binary");
        let sha = hex_digest(&Sha256::digest(&bytes));

        install_bytes(&bytes, &sha, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new binary");
        assert!(!dir.join("pulse.new").exists(), "the staged file is gone");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_verified_appimage_replaces_the_file() {
        let dir = staged_dir("appimage");
        let target = dir.join("Pulse-x86_64.AppImage");
        std::fs::write(&target, b"old image").unwrap();
        let bytes = b"new image bytes";
        let sha = hex_digest(&Sha256::digest(bytes));

        install_appimage(bytes, &sha, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new image bytes");
        assert!(!dir.join("Pulse-x86_64.AppImage.new").exists());
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_appimage_changes_nothing() {
        let dir = staged_dir("appimage-badsha");
        let target = dir.join("Pulse-x86_64.AppImage");
        std::fs::write(&target, b"old image").unwrap();

        let error = install_appimage(b"new image bytes", &"0".repeat(64), &target).unwrap_err();
        assert!(error.contains("corrupted"));
        assert_eq!(std::fs::read(&target).unwrap(), b"old image");
        assert!(!dir.join("Pulse-x86_64.AppImage.new").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_checksum_changes_nothing() {
        let dir = staged_dir("badsha");
        let target = dir.join("pulse");
        std::fs::write(&target, b"old").unwrap();
        let bytes = archive(b"new binary");

        let error = install_bytes(&bytes, &"0".repeat(64), &target).unwrap_err();
        assert!(error.contains("corrupted"));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
