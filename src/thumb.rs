//! Thumbnails of pictures and videos for the preview pane, made off the UI thread.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Longest side of a thumbnail, in pixels.
const SIDE: u32 = 640;
/// Thumbnails kept in memory.
const KEEP: usize = 24;

const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "ico", "icns", "psd", "svg", "raw", "cr2", "nef", "arw", "dng"];
const VIDEOS: &[&str] = &["mov", "mp4", "m4v", "mkv", "avi", "webm", "mpg", "mpeg", "wmv", "flv", "3gp", "mts", "m2ts"];
const DOCUMENTS: &[&str] = &["pdf"];

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
    if cfg!(target_os = "macos") {
        [PICTURES, VIDEOS, DOCUMENTS].concat().contains(&ext.as_str())
    } else {
        image::ImageFormat::from_extension(&ext).map(|f| f.reading_enabled()).unwrap_or(false)
    }
}

/// Quick Look makes the thumbnail, so every kind the system previews is covered.
#[cfg(target_os = "macos")]
fn render(path: &Path) -> Option<egui::ColorImage> {
    use std::os::darwin::fs::MetadataExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    /// Set on a file whose content is still in the cloud: reading it would download it.
    const SF_DATALESS: u32 = 0x4000_0000;
    extern "C" {
        fn getpid() -> i32;
    }

    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.st_flags() & SF_DATALESS != 0 {
        return None;
    }
    // SAFETY: getpid has no preconditions.
    let dir = std::env::temp_dir().join(format!("sunburst-thumbs-{}", unsafe { getpid() }));
    std::fs::create_dir_all(&dir).ok()?;
    let mut child = Command::new("qlmanage")
        .args(["-t", "-s", &SIDE.to_string(), "-o"])
        .arg(&dir)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut name = path.file_name()?.to_os_string();
    name.push(".png");
    let out = dir.join(name);
    let picture = image::open(&out).ok();
    let _ = std::fs::remove_file(&out);
    picture.map(to_pixels)
}

#[cfg(not(target_os = "macos"))]
fn render(path: &Path) -> Option<egui::ColorImage> {
    // A picture far larger than memory allows is not worth a thumbnail.
    if std::fs::metadata(path).ok()?.len() > 200_000_000 {
        return None;
    }
    image::open(path).ok().map(to_pixels)
}

fn to_pixels(picture: image::DynamicImage) -> egui::ColorImage {
    let small = picture.thumbnail(SIDE, SIDE).to_rgba8();
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
        assert_eq!(has_thumbnail(Path::new("/x/clip.mov")), cfg!(target_os = "macos"));
    }

    #[test]
    fn renders_a_picture() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dot.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([200, 30, 30, 255])).save(&file).unwrap();
        let pixels = render(&file).expect("thumbnail");
        assert!(pixels.size[0] >= pixels.size[1] && pixels.size[0] <= SIDE as usize);
        assert!(render(&dir.path().join("missing.png")).is_none());
    }
}
