//! Looks for a newer release on GitHub and downloads it for the user to install.
//!
//! Nothing is replaced in place: the disk image or archive of this system is
//! saved in the Downloads folder, checked against the release's
//! SHA256SUMS.txt, and the user swaps the application themselves.

use crate::i18n::{t, tf};
use parking_lot::Mutex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A release answer is a few kilobytes; the checksum list one line per file.
const MAX_ANSWER: u64 = 1 << 20;
const MAX_SUMS: u64 = 64 << 10;
const MAX_NOTES: usize = 4000;
/// File types of the published downloads, longest first.
const EXTENSIONS: &[&str] = &[".tar.gz", ".AppImage", ".dmg", ".zip"];

fn repository() -> &'static str {
    env!("CARGO_PKG_REPOSITORY").trim_end_matches('/')
}

/// `owner/name` of the GitHub repository.
fn slug() -> &'static str {
    repository().trim_start_matches("https://github.com/")
}

pub fn releases_page() -> String {
    format!("{}/releases/latest", repository())
}

/// `1.2.3` or `v1.2` as numbers; pre-releases (`1.2.0-beta`) are not offered.
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let mut parts = text.split('.').map(|p| p.parse::<u64>().ok());
    let version = [parts.next()??, parts.next().unwrap_or(Some(0))?, parts.next().unwrap_or(Some(0))?];
    parts.next().is_none().then_some(version)
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    url: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Release {
    /// Without the leading `v`.
    pub version: String,
    pub page: String,
    pub notes: String,
    /// The download for this system, when the release has one.
    pub asset: Option<Asset>,
    sums: Option<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// End of the download name for this system, after the version.
fn asset_suffix() -> Option<String> {
    if cfg!(target_os = "macos") {
        Some("-macos-universal.dmg".into())
    } else if cfg!(windows) {
        Some("-windows-x64.zip".into())
    } else if cfg!(target_os = "linux") {
        let kind = if std::env::var_os("APPIMAGE").is_some() { "AppImage" } else { "tar.gz" };
        Some(format!("-linux-{}.{kind}", std::env::consts::ARCH))
    } else {
        None
    }
}

/// Only plain file names from this repository's releases are downloaded.
fn usable(asset: &ApiAsset) -> bool {
    let prefix = format!("https://github.com/{}/releases/download/", slug());
    !asset.name.is_empty()
        && !asset.name.starts_with('.')
        && asset.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && asset.browser_download_url.starts_with(&prefix)
        && asset.browser_download_url.ends_with(&format!("/{}", asset.name))
}

/// The release described by a GitHub answer, when it is newer than `current`.
fn parse_release(json: &str, current: &str, suffix: Option<&str>) -> Result<Option<Release>, String> {
    let api: ApiRelease = serde_json::from_str(json).map_err(|_| t("GitHub sent an unexpected answer.").to_string())?;
    let (Some(latest), Some(current)) = (parse_version(&api.tag_name), parse_version(current)) else {
        return Ok(None);
    };
    if api.draft || api.prerelease || latest <= current {
        return Ok(None);
    }
    let page_prefix = format!("{}/releases/", repository());
    let page = if api.html_url.starts_with(&page_prefix) { api.html_url } else { releases_page() };
    let find = |wanted: &dyn Fn(&str) -> bool| api.assets.iter().find(|a| usable(a) && wanted(&a.name));
    let asset = suffix
        .and_then(|s| find(&|name: &str| name.ends_with(s)))
        .map(|a| Asset { name: a.name.clone(), url: a.browser_download_url.clone(), size: a.size });
    let sums = find(&|name: &str| name == "SHA256SUMS.txt").map(|a| a.browser_download_url.clone());
    let mut notes = plain_notes(&api.body.unwrap_or_default());
    if notes.len() > MAX_NOTES {
        let mut end = MAX_NOTES;
        while !notes.is_char_boundary(end) {
            end -= 1;
        }
        notes.truncate(end);
        notes.push('…');
    }
    let version = format!("{}.{}.{}", latest[0], latest[1], latest[2]);
    Ok(Some(Release { version, page, notes, asset, sums }))
}

/// Release notes are Markdown; the dialog shows them as plain text.
fn plain_notes(markdown: &str) -> String {
    let lines = markdown.lines().map(|line| {
        let line = line.trim_end().replace("**", "").replace('`', "");
        let bare = line.trim_start();
        if let Some(item) = bare.strip_prefix("* ").or_else(|| bare.strip_prefix("- ")) {
            format!("• {item}")
        } else {
            bare.trim_start_matches('#').trim_start().to_string()
        }
    });
    lines.collect::<Vec<_>>().join("\n").trim().to_string()
}

fn agent(body_timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .user_agent(concat!("sunburst-disk-analyser/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(body_timeout))
        .build()
        .into()
}

fn unreachable(e: impl std::fmt::Display) -> String {
    tf("Could not reach GitHub: {}", &[&e.to_string()])
}

fn fetch_text(agent: &ureq::Agent, url: &str, limit: u64) -> Result<String, String> {
    let mut request = agent.get(url);
    if url.starts_with("https://api.github.com/") {
        request = request.header("Accept", "application/vnd.github+json");
    }
    let mut answer = request.call().map_err(unreachable)?;
    answer.body_mut().with_config().limit(limit).read_to_string().map_err(unreachable)
}

pub enum Outcome {
    UpToDate,
    Available(Release),
    Failed(String),
}

/// A check running in the background.
pub struct Check(Arc<Mutex<Option<Outcome>>>);

impl Check {
    pub fn take(&self) -> Option<Outcome> {
        self.0.lock().take()
    }
}

/// Asks GitHub for the latest release, without waiting for the answer.
pub fn check(ctx: egui::Context) -> Check {
    let slot = Arc::new(Mutex::new(None));
    let out = slot.clone();
    std::thread::spawn(move || {
        let url = format!("https://api.github.com/repos/{}/releases/latest", slug());
        let outcome = fetch_text(&agent(Duration::from_secs(15)), &url, MAX_ANSWER)
            .and_then(|json| parse_release(&json, env!("CARGO_PKG_VERSION"), asset_suffix().as_deref()));
        *out.lock() = Some(match outcome {
            Ok(Some(release)) => Outcome::Available(release),
            Ok(None) => Outcome::UpToDate,
            Err(e) => Outcome::Failed(e),
        });
        ctx.request_repaint();
    });
    Check(slot)
}

/// A download running in the background.
pub struct Download {
    pub done: AtomicU64,
    pub total: u64,
    cancel: AtomicBool,
    result: Mutex<Option<Result<PathBuf, String>>>,
}

impl Download {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn take(&self) -> Option<Result<PathBuf, String>> {
        self.result.lock().take()
    }
}

/// Saves the release's download for this system in the Downloads folder.
pub fn download(release: &Release, ctx: egui::Context) -> Option<Arc<Download>> {
    let asset = release.asset.clone()?;
    let sums = release.sums.clone();
    let job = Arc::new(Download { done: AtomicU64::new(0), total: asset.size, cancel: AtomicBool::new(false), result: Mutex::new(None) });
    let out = job.clone();
    std::thread::spawn(move || {
        let result = dirs::download_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| t("No Downloads folder was found.").to_string())
            .and_then(|dir| fetch_checked(&asset, sums.as_deref(), &out, &dir));
        *out.result.lock() = Some(result);
        ctx.request_repaint();
    });
    Some(job)
}

/// The SHA-256 the checksum list gives for `name`, in lower case.
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_ascii_lowercase())
    })
}

fn fetch_checked(asset: &Asset, sums: Option<&str>, job: &Download, dir: &Path) -> Result<PathBuf, String> {
    let agent = agent(Duration::from_secs(30 * 60));
    let no_sum = || t("The release has no checksum for this file.").to_string();
    let sums = fetch_text(&agent, sums.ok_or_else(no_sum)?, MAX_SUMS)?;
    let expected = expected_hash(&sums, &asset.name).ok_or_else(no_sum)?;
    let temp = dir.join(format!(".{}.{}.part", asset.name, std::process::id()));
    let saved = save(&agent, asset, &expected, &temp, job).and_then(|()| publish(&temp, dir, &asset.name).map_err(|e| e.to_string()));
    if saved.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    saved
}

/// Writes the download to `temp` and checks its length and checksum.
fn save(agent: &ureq::Agent, asset: &Asset, expected: &str, temp: &Path, job: &Download) -> Result<(), String> {
    let mut answer = agent.get(&asset.url).call().map_err(unreachable)?;
    let mut body = answer.body_mut().with_config().limit(asset.size + 1).reader();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // An AppImage is the program itself.
        options.mode(if asset.name.ends_with(".AppImage") { 0o755 } else { 0o644 });
    }
    let mut file = options.open(temp).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    let mut done = 0u64;
    loop {
        if job.cancel.load(Ordering::Acquire) {
            return Err(t("The download was stopped.").into());
        }
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(unreachable(e)),
        };
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        job.done.store(done, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if done != asset.size || hash != expected {
        return Err(t("The downloaded file is damaged: its checksum does not match. It was deleted.").into());
    }
    Ok(())
}

/// `name`, then `name (2)`, `name (3)`… keeping the file type at the end.
fn numbered(name: &str, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    let ext = EXTENSIONS.iter().find(|e| name.ends_with(*e)).copied().unwrap_or("");
    format!("{} ({n}){ext}", &name[..name.len() - ext.len()])
}

/// Renames `temp` into `dir` under the first free name; an existing file is
/// never replaced, since the name is reserved before the rename.
fn publish(temp: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    for n in 1..1000 {
        let target = dir.join(numbered(name, n));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        return match std::fs::rename(temp, &target) {
            Ok(()) => Ok(target),
            Err(e) => {
                let _ = std::fs::remove_file(&target);
                Err(e)
            }
        };
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// What to do with the saved file, for this system.
pub fn install_steps(file: &Path) -> &'static str {
    let name = file.to_string_lossy();
    if cfg!(target_os = "macos") {
        t("Quit this app, open the disk image and drag Sunburst Disk Analyser to Applications to replace the old version. macOS may then ask again for Full Disk Access.")
    } else if cfg!(windows) {
        t("Quit this app, extract the archive and replace the old application files with the new ones.")
    } else if name.ends_with(".AppImage") {
        t("Quit this app and replace the old AppImage with the new one.")
    } else {
        t("Quit this app, extract the archive and replace the old program file with the new one.")
    }
}

/// A made-up newer release, for the screenshot of the update dialog.
pub fn sample_release() -> Release {
    let version = "1.1.0".to_string();
    let name = format!("Sunburst-Disk-Analyser-{version}{}", asset_suffix().unwrap_or_default());
    Release {
        page: format!("{}/releases/tag/v{version}", repository()),
        notes: plain_notes("## What's Changed\n* Faster scans of large folders\n* Update check at launch"),
        asset: Some(Asset { name, url: String::new(), size: 9_800_000 }),
        sums: None,
        version,
    }
}

/// Where the update dialog is.
pub enum Stage {
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, Arc<Download>),
    Downloaded(PathBuf),
    /// Title, explanation.
    Failed(&'static str, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(tag: &str, assets: &[&str]) -> String {
        let base = format!("https://github.com/{}/releases/download/{tag}", slug());
        let assets: Vec<_> = assets
            .iter()
            .map(|name| serde_json::json!({ "name": name, "browser_download_url": format!("{base}/{name}"), "size": 10 }))
            .collect();
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("{}/releases/tag/{tag}", repository()),
            "body": "Notes",
            "assets": assets,
        })
        .to_string()
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(parse_version("v1.10.0"), Some([1, 10, 0]));
        assert_eq!(parse_version("2.1"), Some([2, 1, 0]));
        assert!(parse_version("v1.10.0") > parse_version("1.9.9"));
        assert_eq!(parse_version("1.2.0-beta"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn only_newer_releases_are_offered() {
        let names = ["Sunburst-Disk-Analyser-1.1.0-macos-universal.dmg", "Sunburst-Disk-Analyser-1.1.0-windows-x64.zip", "SHA256SUMS.txt"];
        let json = answer("v1.1.0", &names);
        assert!(parse_release(&json, "1.1.0", None).unwrap().is_none());
        assert!(parse_release(&json, "1.2.0", None).unwrap().is_none());
        let release = parse_release(&json, "1.0.0", Some("-windows-x64.zip")).unwrap().unwrap();
        assert_eq!(release.version, "1.1.0");
        assert_eq!(release.asset.unwrap().name, names[1]);
        assert!(release.sums.is_some());
        assert!(release.page.ends_with("/releases/tag/v1.1.0"));
        // No download for this system: only the release page is offered.
        let release = parse_release(&json, "1.0.0", Some("-linux-aarch64.AppImage")).unwrap().unwrap();
        assert!(release.asset.is_none());
        assert!(parse_release("{}", "1.0.0", None).is_err());
    }

    #[test]
    fn downloads_come_from_the_repository_only() {
        let mut json: serde_json::Value = serde_json::from_str(&answer("v2.0.0", &["App-2.0.0-windows-x64.zip"])).unwrap();
        json["assets"][0]["browser_download_url"] = "https://example.com/App-2.0.0-windows-x64.zip".into();
        json["html_url"] = "https://example.com/".into();
        let release = parse_release(&json.to_string(), "1.0.0", Some("-windows-x64.zip")).unwrap().unwrap();
        assert!(release.asset.is_none());
        assert_eq!(release.page, releases_page());
        let release = parse_release(&answer("v2.0.0", &["../App-windows-x64.zip"]), "1.0.0", Some("-windows-x64.zip")).unwrap().unwrap();
        assert!(release.asset.is_none());
    }

    #[test]
    fn notes_lose_their_markdown() {
        let notes = "## What's Changed\r\n* **Faster** scans by @a in #3\r\n\r\n**Full Changelog**: `v1...v2`";
        assert_eq!(plain_notes(notes), "What's Changed\n• Faster scans by @a in #3\n\nFull Changelog: v1...v2");
    }

    #[test]
    fn checksums_are_found_by_file_name() {
        let hash = "ab".repeat(32);
        let sums = format!("{}  other.zip\n{}  App.dmg\n", "cd".repeat(32), hash.to_uppercase());
        assert_eq!(expected_hash(&sums, "App.dmg"), Some(hash));
        assert_eq!(expected_hash(&sums, "App.zip"), None);
        assert_eq!(expected_hash("abc  App.dmg", "App.dmg"), None);
    }

    #[test]
    fn saved_files_never_replace_existing_ones() {
        assert_eq!(numbered("App-1.1.0-linux-x86_64.tar.gz", 2), "App-1.1.0-linux-x86_64 (2).tar.gz");
        assert_eq!(numbered("App.dmg", 1), "App.dmg");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("App.dmg"), b"old").unwrap();
        let temp = dir.path().join(".App.dmg.part");
        std::fs::write(&temp, b"new").unwrap();
        let saved = publish(&temp, dir.path(), "App.dmg").unwrap();
        assert_eq!(saved, dir.path().join("App (2).dmg"));
        assert_eq!(std::fs::read(dir.path().join("App.dmg")).unwrap(), b"old");
        assert_eq!(std::fs::read(&saved).unwrap(), b"new");
        assert!(!temp.exists());
    }

    /// Talks to GitHub: `cargo test -- --ignored published_release_downloads`.
    #[test]
    #[ignore]
    fn published_release_downloads_and_checks_out() {
        let url = format!("https://api.github.com/repos/{}/releases/latest", slug());
        let json = fetch_text(&agent(Duration::from_secs(15)), &url, MAX_ANSWER).unwrap();
        let release = parse_release(&json, "0.0.1", asset_suffix().as_deref()).unwrap().unwrap();
        let asset = release.asset.clone().expect("a download for this system");
        let job = Download { done: AtomicU64::new(0), total: asset.size, cancel: AtomicBool::new(false), result: Mutex::new(None) };
        let dir = tempfile::tempdir().unwrap();
        let file = fetch_checked(&asset, release.sums.as_deref(), &job, dir.path()).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().len(), asset.size);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no partial file is left");
        // A wrong checksum list is refused and leaves nothing behind.
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let fake = Asset { name: "SHA256SUMS.txt".into(), ..asset.clone() };
        assert!(fetch_checked(&fake, release.sums.as_deref(), &job, &other).is_err());
        assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
    }
}
