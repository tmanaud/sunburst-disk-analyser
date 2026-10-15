//! Synthetic, deterministic disk content used for screenshots (`--demo`),
//! so that no real file name ever appears in the documentation.

use crate::scan::{Feed, ScanHandle};
use crate::tree::*;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
    fn unit(&mut self) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0
    }
}

const DIRS: &[&str] = &[
    "archive", "assets", "backups", "build", "cache", "clients", "datasets", "design", "exports", "footage", "invoices",
    "library", "models", "node_modules", "notes", "old", "raw", "renders", "samples", "sessions", "shared", "src",
    "target", "textures", "tmp", "vendor", "2023", "2024", "2025", "2026",
];
const FILES: &[&str] = &[
    "report.pdf", "final-cut.mov", "interview.mp4", "dataset.parquet", "backup.zip", "disk.img", "model.gguf",
    "photo-0412.raw", "mix-master.wav", "album.flac", "slides.key", "budget.xlsx", "scan.tiff", "trailer.mkv",
    "installer.dmg", "notes.md", "index.db", "render-0090.exr", "podcast.mp3", "archive.tar.gz", "sketch.psd",
    "capture.pcap", "firmware.bin", "manual.pdf", "timelapse.mp4", "export.csv",
];

const DAY: i64 = 86_400;
const BASE_TIME: i64 = 1_788_000_000;

/// Splits `total` in `parts` uneven shares, largest first.
fn split(rng: &mut Rng, total: u64, parts: usize) -> Vec<u64> {
    let weights: Vec<f64> = (0..parts).map(|i| (0.25 + rng.unit()) / (1.0 + i as f64 * 1.6)).collect();
    let sum: f64 = weights.iter().sum();
    weights.iter().map(|w| ((w / sum) * total as f64) as u64).collect()
}

/// Fills `dir` with `size` bytes of made-up content, plus the `extra` files.
fn fill(tree: &mut Tree, rng: &mut Rng, dir: NodeId, size: u64, depth: u32, extra: &[(&str, u64)]) {
    let size = size - extra.iter().map(|e| e.1).sum::<u64>();
    let n_dirs = if depth >= 6 || size < 300_000_000 { 0 } else { rng.range(2, 5) as usize };
    let n_files = rng.range(2, 6) as usize;
    let dir_share = if n_dirs == 0 { 0 } else { (size as f64 * (0.55 + rng.unit() * 0.35)) as u64 };
    let mut entries = Vec::new();
    let mut dir_sizes = Vec::new();
    let mut pick = rng.range(0, 1000) as usize;
    for s in split(rng, dir_share, n_dirs) {
        pick += 1 + rng.range(0, 3) as usize;
        entries.push(Entry { name: DIRS[pick % DIRS.len()].into(), size: 0, mtime: BASE_TIME - rng.range(1, 700) as i64 * DAY, flags: F_DIR });
        dir_sizes.push(s);
    }
    for (i, s) in split(rng, size - dir_share, n_files).into_iter().enumerate() {
        pick += 1 + rng.range(0, 4) as usize;
        let base = FILES[pick % FILES.len()];
        let name = if i < 3 { base.to_string() } else { format!("{}-{}", i, base) };
        entries.push(Entry { name: name.into(), size: s, mtime: BASE_TIME - rng.range(1, 900) as i64 * DAY, flags: 0 });
    }
    for (name, s) in extra {
        entries.push(Entry { name: (*name).into(), size: *s, mtime: BASE_TIME - 40 * DAY, flags: 0 });
    }
    let first = tree.add_children(dir, entries);
    for (i, s) in dir_sizes.into_iter().enumerate() {
        fill(tree, rng, first + i as u32, s, depth + 1, &[]);
    }
}

pub const HOME_NAME: &str = "alex";

/// A film with a picture for the preview pane: Big Buck Bunny,
/// (c) copyright 2008 Blender Foundation, www.bigbuckbunny.org, CC BY 3.0.
pub const MOVIE: &str = "Big Buck Bunny 4K.mov";
const MOVIE_PICTURE: &str = "big-buck-bunny.jpg";

pub fn home_path() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "C:\\Users\\alex" } else { "/Users/alex" })
}

pub fn tree() -> Tree {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    let mut tree = Tree::new(home_path());
    tree.nodes[0].mtime = BASE_TIME;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let top: &[(&str, u64, u8)] = &[
        ("Projects", 312 * GB, F_DIR),
        ("Videos collection", 283 * GB, F_DIR),
        ("Movies", 199 * GB, F_DIR),
        ("Virtual Machines", 99 * GB, F_DIR),
        ("Pictures", 98 * GB, F_DIR),
        ("Music", 63 * GB + 100 * MB, F_DIR),
        ("Documents", 37 * GB + 400 * MB, F_DIR),
        ("Applications", 33 * GB + 700 * MB, F_DIR),
        ("Downloads", 6_560 * MB, F_DIR),
        ("Toolchains", 4_280 * MB, F_DIR),
        ("site-backup", 891 * MB, F_DIR),
        ("firmware", 684 * MB, F_DIR),
        ("Public", 295 * MB, F_DIR),
        ("labels", 132 * MB, F_DIR),
        ("handbook-2026.pdf", 59_300_000, 0),
        ("update-0064.bin", 22_400_000, 0),
        ("Screen Recording 14.23.mov", 6_740_000, 0),
        ("todo.txt", 4_096, 0),
        ("Library", 410 * GB, F_DIR | F_HIDDEN),
        (".cache", 16 * GB + 400 * MB, F_DIR | F_HIDDEN),
        (".rustup", 3_130 * MB, F_DIR | F_HIDDEN),
        (".npm", 2_590 * MB, F_DIR | F_HIDDEN),
        (".config", 48 * MB, F_DIR | F_HIDDEN),
        (".zsh_history", 180_000, F_HIDDEN),
    ];
    let entries = top
        .iter()
        .enumerate()
        .map(|(i, (name, size, flags))| Entry {
            name: (*name).into(),
            size: if flags & F_DIR != 0 { 0 } else { *size },
            mtime: BASE_TIME - (i as i64 * 11 + 2) * DAY,
            flags: *flags,
        })
        .collect();
    let first = tree.add_children(0, entries);
    for (i, (name, size, flags)) in top.iter().enumerate() {
        if flags & F_DIR != 0 {
            let extra: &[(&str, u64)] = if *name == "Movies" { &[(MOVIE, 41_600_000_000)] } else { &[] };
            fill(&mut tree, &mut rng, first + i as u32, *size, 1, extra);
        }
    }
    tree
}

/// The picture of a demo file, from the folder given with `--demo-media`.
pub fn picture(media: &Path, file: &Path) -> Option<egui::ColorImage> {
    if file.file_name()? != MOVIE {
        return None;
    }
    let picture = image::open(media.join(MOVIE_PICTURE)).ok()?.to_rgba8();
    let size = [picture.width() as usize, picture.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, &picture))
}

/// Plays the demo tree back as if it was being scanned, one folder at a time
/// and level by level, for the recording of the README animation.
pub struct Replay {
    full: Tree,
    feed: Feed,
    /// Folders still to read: id in `full`, id in the scanned tree.
    queue: VecDeque<(NodeId, NodeId)>,
    folders: usize,
    read: usize,
}

impl Replay {
    pub fn start() -> (ScanHandle, Replay) {
        let full = tree();
        let folders = full.nodes.iter().filter(|n| n.is_dir()).count();
        let (handle, feed) = ScanHandle::scripted(home_path());
        handle.tree.write().nodes[0].mtime = BASE_TIME;
        (handle, Replay { full, feed, queue: VecDeque::from([(0, 0)]), folders, read: 0 })
    }

    /// Reads folders until `share` of them, 0 to 1, are read.
    pub fn advance(&mut self, share: f64) {
        let want = ((share.clamp(0.0, 1.0) * self.folders as f64).ceil() as usize).min(self.folders);
        let mut tree = self.feed.tree.write();
        while self.read < want {
            let Some((from, to)) = self.queue.pop_front() else { break };
            let entries = self
                .full
                .children(from)
                .map(|id| {
                    let n = self.full.node(id);
                    Entry { name: n.name.to_os_string(), size: if n.is_dir() { 0 } else { n.size }, mtime: n.mtime, flags: n.flags }
                })
                .collect();
            let start = tree.add_children(to, entries);
            for (i, id) in self.full.children(from).enumerate() {
                if self.full.node(id).is_dir() {
                    self.queue.push_back((id, start + i as u32));
                }
            }
            self.read += 1;
        }
        let (entries, bytes) = (tree.nodes.len() as u64 - 1, tree.node(0).size);
        drop(tree);
        self.feed.report(entries, bytes, self.read as f64 / self.folders as f64);
    }

    pub fn finish(&mut self, elapsed: Duration) {
        self.advance(1.0);
        self.feed.finish(elapsed);
    }

    #[cfg(test)]
    fn is_finished(&self) -> bool {
        self.read >= self.folders
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_tree_is_stable() {
        let a = super::tree();
        let b = super::tree();
        assert_eq!(a.len(), b.len());
        assert_eq!(a.node(0).size, b.node(0).size);
        assert!(a.len() > 1000);
        assert!(a.node(0).size > 1_400_000_000_000);
    }

    #[test]
    fn replay_ends_with_the_whole_tree() {
        let full = super::tree();
        let (handle, mut replay) = super::Replay::start();
        replay.advance(0.3);
        assert!(!replay.is_finished());
        assert!(handle.tree.read().node(0).size < full.node(0).size);
        replay.finish(std::time::Duration::from_millis(2400));
        assert!(handle.is_done());
        let tree = handle.tree.read();
        assert_eq!(tree.len(), full.len());
        assert_eq!(tree.node(0).size, full.node(0).size);
        assert_eq!(handle.bytes(), full.node(0).size);
    }
}
