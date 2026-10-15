//! Thumbnails of pictures and videos for the preview pane, made off the UI thread.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

/// Longest side of a thumbnail, in pixels.
const SIDE: u32 = 640;
/// Thumbnails kept in memory.
const KEEP: usize = 24;
/// Longest wait for a thumbnail; past it the file shows its icon instead.
const WAIT: Duration = Duration::from_secs(6);

const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "ico", "icns", "psd", "svg", "raw", "cr2", "nef", "arw", "dng"];
const VIDEOS: &[&str] = &[
    "mov", "mp4", "m4v", "mkv", "avi", "webm", "mpg", "mpeg", "wmv", "flv", "3gp", "3g2", "mts", "m2ts", "ts", "ogv", "vob", "asf", "divx", "f4v",
];
/// The videos a stock Mac's Quick Look pictures; on the others (mkv, webm, avi…)
/// it hangs without a word, so they go to ffmpeg.
const QUICK_LOOK_VIDEOS: &[&str] = &["mov", "mp4", "m4v", "3gp", "3g2", "mpg", "mpeg", "mts", "m2ts"];
const DOCUMENTS: &[&str] = &["pdf"];
/// The ffmpeg readers of those videos. Formats that open further files
/// (playlists, concat lists, HLS, DASH…) are refused: a file that merely
/// looks like a video could otherwise make ffmpeg read others, cloud-only
/// ones included.
const DEMUXERS: &str = "matroska,mov,avi,asf,flv,mpeg,mpegts,ogg";

type Pixels = (PathBuf, Option<egui::ColorImage>);

pub struct Thumbs {
    /// `None` while a thumbnail is being made, and for files that have none.
    known: HashMap<PathBuf, Option<egui::TextureHandle>>,
    order: VecDeque<PathBuf>,
    tx: mpsc::Sender<Pixels>,
    rx: mpsc::Receiver<Pixels>,
}

impl Default for Thumbs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Thumbs { known: HashMap::new(), order: VecDeque::new(), tx, rx }
    }
}

impl Thumbs {
    /// Starts making the thumbnail of `path` unless that was done before.
    pub fn want(&mut self, ctx: &egui::Context, path: &Path) {
        if self.known.contains_key(path) {
            return;
        }
        self.known.insert(path.to_path_buf(), None);
        self.order.push_back(path.to_path_buf());
        if self.order.len() > KEEP {
            if let Some(old) = self.order.pop_front() {
                self.known.remove(&old);
            }
        }
        if !has_thumbnail(path) {
            return;
        }
        let (tx, ctx, path) = (self.tx.clone(), ctx.clone(), path.to_path_buf());
        std::thread::spawn(move || {
            let pixels = render(&path);
            let _ = tx.send((path, pixels));
            ctx.request_repaint();
        });
    }

    /// Takes in the thumbnails finished since the last frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, pixels)) = self.rx.try_recv() {
            if let (Some(slot), Some(pixels)) = (self.known.get_mut(&path), pixels) {
                *slot = Some(ctx.load_texture(path.to_string_lossy(), pixels, egui::TextureOptions::LINEAR));
            }
        }
    }

    /// Sets the thumbnail of a file that is not on disk (demo data).
    pub fn insert(&mut self, path: PathBuf, texture: egui::TextureHandle) {
        self.known.insert(path, Some(texture));
    }

    pub fn get(&self, path: &Path) -> Option<&egui::TextureHandle> {
        self.known.get(path)?.as_ref()
    }
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Whether the file is of a kind this system can picture.
fn has_thumbnail(path: &Path) -> bool {
    let ext = extension(path);
    let ext = ext.as_str();
    if VIDEOS.contains(&ext) {
        return ffmpeg().is_some() || (cfg!(target_os = "macos") && QUICK_LOOK_VIDEOS.contains(&ext));
    }
    if cfg!(target_os = "macos") {
        PICTURES.contains(&ext) || DOCUMENTS.contains(&ext)
    } else {
        image::ImageFormat::from_extension(ext).map(|f| f.reading_enabled()).unwrap_or(false)
    }
}

/// Videos go to Quick Look when it knows them and to ffmpeg otherwise;
/// files whose content is still in the cloud are never read.
fn render(path: &Path) -> Option<egui::ColorImage> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || cloud_only(&meta) {
        return None;
    }
    let ext = extension(path);
    if VIDEOS.contains(&ext.as_str()) {
        #[cfg(target_os = "macos")]
        if QUICK_LOOK_VIDEOS.contains(&ext.as_str()) {
            if let Some(pixels) = quick_look(path) {
                return Some(pixels);
            }
        }
        return video_frame(path);
    }
    picture(path, &meta)
}

/// Quick Look makes the thumbnail, so every kind the system previews is covered.
#[cfg(target_os = "macos")]
fn picture(path: &Path, _meta: &std::fs::Metadata) -> Option<egui::ColorImage> {
    quick_look(path)
}

#[cfg(not(target_os = "macos"))]
fn picture(path: &Path, meta: &std::fs::Metadata) -> Option<egui::ColorImage> {
    // A picture far larger than memory allows is not worth a thumbnail.
    if meta.len() > 200_000_000 {
        return None;
    }
    image::open(path).ok().map(to_pixels)
}

/// Set on a file whose content is still in the cloud: reading it would download it.
#[cfg(target_os = "macos")]
fn cloud_only(meta: &std::fs::Metadata) -> bool {
    use std::os::darwin::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0
}

/// OneDrive "online-only" placeholders.
#[cfg(windows)]
fn cloud_only(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    meta.file_attributes() & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(any(target_os = "macos", windows)))]
fn cloud_only(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn quick_look(path: &Path) -> Option<egui::ColorImage> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // A folder per thumbnail, as two files of the same name would share an output name.
    let dir = std::env::temp_dir()
        .join(format!("sunburst-thumbs-{}", std::process::id()))
        .join(NEXT.fetch_add(1, Ordering::Relaxed).to_string());
    std::fs::create_dir_all(&dir).ok()?;
    let mut cmd = Command::new("/usr/bin/qlmanage");
    cmd.args(["-t", "-s", &SIDE.to_string(), "-o"]).arg(&dir).arg(path);
    let made = run(cmd, 1 << 16).is_some();
    let mut name = path.file_name()?.to_os_string();
    name.push(".png");
    let picture = if made { image::open(dir.join(name)).ok() } else { None };
    let _ = std::fs::remove_dir_all(&dir);
    picture.map(to_pixels)
}

/// A frame of the video grabbed by an ffmpeg the user installed; none is bundled.
fn video_frame(path: &Path) -> Option<egui::ColorImage> {
    let ffmpeg = ffmpeg()?;
    // "file:" keeps a path from being read as another protocol, and only local files may be opened.
    let mut input = std::ffi::OsString::from("file:");
    input.push(path);
    let scale = format!("scale=w='min({SIDE},iw)':h='min({SIDE},ih)':force_original_aspect_ratio=decrease");
    // A few seconds in, as videos often open on black; a shorter clip yields nothing there.
    for seek in ["3", "0"] {
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-protocol_whitelist", "file", "-format_whitelist", DEMUXERS])
            .args(["-ss", seek, "-i"])
            .arg(&input)
            .args(["-frames:v", "1", "-an", "-sn", "-dn", "-vf", &scale, "-f", "image2pipe", "-c:v", "png", "pipe:1"]);
        let png = run(cmd, 64 << 20)?;
        if !png.is_empty() {
            return image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok().map(to_pixels);
        }
    }
    None
}

/// ffmpeg from the PATH, or from where Homebrew and MacPorts put it: apps
/// opened from the Finder do not get the shell's PATH.
fn ffmpeg() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
            let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
            let usual = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map(PathBuf::from);
            // Relative entries would run whatever ffmpeg lies in the current folder.
            path.into_iter().chain(usual).filter(|d| d.is_absolute()).map(|d| d.join(name)).find(|f| f.is_file())
        })
        .as_deref()
}

/// Runs `cmd` and returns what it printed, up to `limit` bytes; `None` when it
/// fails or takes longer than `WAIT`, as Quick Look does on kinds it cannot read.
/// The wait covers the output too, and whatever the program started is stopped
/// with it: an ffmpeg wrapper script may leave a process holding the output open.
fn run(mut cmd: Command, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let group = Group::of(&child);
    let out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut printed = Vec::new();
        let _ = out.take(limit).read_to_end(&mut printed);
        let _ = tx.send(printed);
    });
    let deadline = Instant::now() + WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => break None,
        }
    };
    let printed = match status {
        Some(s) if s.success() => rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok(),
        _ => None,
    };
    if printed.is_none() {
        group.kill();
        let _ = child.kill();
    }
    let _ = child.wait();
    printed
}

/// The program run for a thumbnail with everything it starts: its own process
/// group on Unix, a job object on Windows.
struct Group {
    #[cfg(unix)]
    id: i32,
    #[cfg(windows)]
    job: *mut std::ffi::c_void,
}

#[cfg(windows)]
extern "system" {
    fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
    fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
    fn TerminateJobObject(job: *mut std::ffi::c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

impl Group {
    #[cfg(unix)]
    fn of(child: &std::process::Child) -> Group {
        Group { id: child.id() as i32 }
    }

    #[cfg(windows)]
    fn of(child: &std::process::Child) -> Group {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: plain Win32 calls on a live process handle; a null job is never used.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if !job.is_null() && AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                CloseHandle(job);
                return Group { job: std::ptr::null_mut() };
            }
            Group { job }
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn of(_child: &std::process::Child) -> Group {
        Group {}
    }

    /// Called while the program is still running or has left processes behind,
    /// so the group still exists and its id cannot belong to anything else.
    fn kill(&self) {
        #[cfg(unix)]
        // SAFETY: kill has no memory preconditions; the group is the child's own.
        unsafe {
            libc::kill(-self.id, libc::SIGKILL);
        }
        #[cfg(windows)]
        if !self.job.is_null() {
            // SAFETY: the job handle is owned by this value and still open.
            unsafe {
                TerminateJobObject(self.job, 1);
            }
        }
    }
}

#[cfg(windows)]
impl Drop for Group {
    fn drop(&mut self) {
        if !self.job.is_null() {
            // SAFETY: closed once, here.
            unsafe {
                CloseHandle(self.job);
            }
        }
    }
}

fn to_pixels(picture: image::DynamicImage) -> egui::ColorImage {
    // Shrunk only: `thumbnail` would also blow a small picture up.
    let small = if picture.width() > SIDE || picture.height() > SIDE { picture.thumbnail(SIDE, SIDE) } else { picture }.to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_with_a_thumbnail() {
        assert!(has_thumbnail(Path::new("/x/Photo.PNG")));
        assert!(!has_thumbnail(Path::new("/x/archive.zip")));
        assert!(!has_thumbnail(Path::new("/x/no-extension")));
        let ffmpeg = ffmpeg().is_some();
        assert_eq!(has_thumbnail(Path::new("/x/clip.mov")), ffmpeg || cfg!(target_os = "macos"));
        assert_eq!(has_thumbnail(Path::new("/x/clip.mkv")), ffmpeg);
    }

    /// Quick Look hangs on mkv, so the frame must come from ffmpeg, and fast.
    #[test]
    fn renders_a_video_frame() {
        let Some(ffmpeg) = ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        for (name, seconds) in [("clip.mkv", "5"), ("short.mkv", "1")] {
            let file = dir.path().join(name);
            let made = Command::new(ffmpeg)
                .args(["-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(format!("testsrc=duration={seconds}:size=320x180:rate=10"))
                .arg(&file)
                .status()
                .unwrap();
            assert!(made.success());
            let started = Instant::now();
            let pixels = render(&file).expect("thumbnail");
            assert_eq!(pixels.size, [320, 180]);
            assert!(started.elapsed() < WAIT);
        }
    }

    /// A text file named like a video must not make ffmpeg open other files.
    #[test]
    fn video_playlists_are_refused() {
        let Some(ffmpeg) = ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.mkv");
        let made = Command::new(ffmpeg)
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "testsrc=duration=1:size=160x90:rate=10"])
            .arg(&real)
            .status()
            .unwrap();
        assert!(made.success());
        assert!(render(&real).is_some());
        let fake = dir.path().join("fake.mkv");
        std::fs::write(&fake, "ffconcat version 1.0\nfile 'real.mkv'\n").unwrap();
        assert!(render(&fake).is_none());
    }

    /// A program that leaves a process holding its output open is not waited for.
    #[cfg(unix)]
    #[test]
    fn left_behind_processes_do_not_hold_the_thumbnail() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30 & exit 0"]);
        let started = Instant::now();
        assert!(run(cmd, 1024).is_none());
        assert!(started.elapsed() < WAIT + Duration::from_secs(2));
    }

    #[test]
    fn renders_a_picture() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dot.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([200, 30, 30, 255])).save(&file).unwrap();
        let pixels = render(&file).expect("thumbnail");
        assert_eq!(pixels.size, [40, 20]);
        assert!(render(&dir.path().join("missing.png")).is_none());
    }
}
