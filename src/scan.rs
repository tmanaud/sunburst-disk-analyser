//! Parallel directory scanner.
//!
//! Worker threads pull directories from a shared stack, read them and append
//! their entries to the shared [`Tree`] one directory at a time, so the UI can
//! show results while the scan is still running.

#[cfg(unix)]
use crate::pinned::{Dir, Kind};
use crate::tree::*;
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct ScanHandle {
    pub tree: Arc<RwLock<Tree>>,
    pub entries: Arc<AtomicU64>,
    pub started: Instant,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    elapsed_ms: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    progress: Arc<AtomicU64>,
}

impl ScanHandle {
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn was_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn elapsed(&self) -> Duration {
        if self.is_done() {
            Duration::from_millis(self.elapsed_ms.load(Ordering::Relaxed))
        } else {
            self.started.elapsed()
        }
    }
    /// Space taken by the files found so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
    /// Share of the folder tree that has been read, 0 to 1. The total is not
    /// known beforehand, so this is an estimate: see [`WHOLE`].
    pub fn progress(&self) -> f32 {
        (self.progress.load(Ordering::Relaxed) as f64 / WHOLE as f64) as f32
    }
    /// A handle around an already complete tree (demo data, tests).
    pub fn finished(tree: Tree) -> Self {
        ScanHandle {
            tree: Arc::new(RwLock::new(tree)),
            entries: Arc::new(AtomicU64::new(0)),
            started: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(true)),
            elapsed_ms: Arc::new(AtomicU64::new(0)),
            bytes: Arc::new(AtomicU64::new(0)),
            progress: Arc::new(AtomicU64::new(WHOLE)),
        }
    }
    /// A scan whose tree and counters are filled by the caller (demo recording).
    pub fn scripted(root: PathBuf) -> (Self, Feed) {
        let handle = ScanHandle::finished(Tree::new(root));
        handle.done.store(false, Ordering::Release);
        handle.progress.store(0, Ordering::Relaxed);
        let feed = Feed {
            tree: handle.tree.clone(),
            entries: handle.entries.clone(),
            done: handle.done.clone(),
            elapsed_ms: handle.elapsed_ms.clone(),
            bytes: handle.bytes.clone(),
            progress: handle.progress.clone(),
        };
        (handle, feed)
    }
}

/// The writing end of a [`ScanHandle::scripted`] scan.
pub struct Feed {
    pub tree: Arc<RwLock<Tree>>,
    entries: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    elapsed_ms: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    progress: Arc<AtomicU64>,
}

impl Feed {
    /// Reports the counters; `progress` goes from 0 to 1.
    pub fn report(&self, entries: u64, bytes: u64, progress: f64) {
        self.entries.store(entries, Ordering::Relaxed);
        self.bytes.store(bytes, Ordering::Relaxed);
        self.progress.store((progress.clamp(0.0, 1.0) * WHOLE as f64) as u64, Ordering::Relaxed);
    }
    pub fn finish(&self, elapsed: Duration) {
        self.elapsed_ms.store(elapsed.as_millis() as u64, Ordering::Relaxed);
        self.done.store(true, Ordering::Release);
    }
}

/// Progress is counted in shares of the scan. The scanned location holds the
/// whole, every folder splits its share equally between its subfolders, and
/// a share counts as done once the folder holding it has been read.
const WHOLE: u64 = 1 << 62;

/// A folder to read, with the (device, inode) its parent listed it under and
/// its share of the scan.
type Job = (NodeId, PathBuf, Option<(u64, u64)>, u64);

struct Shared {
    tree: Arc<RwLock<Tree>>,
    queue: Mutex<Vec<Job>>,
    cv: Condvar,
    /// Directories queued or being processed.
    pending: AtomicUsize,
    cancel: Arc<AtomicBool>,
    entries: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    progress: Arc<AtomicU64>,
    /// (device, inode) of multiply linked files already counted.
    #[cfg_attr(not(unix), allow(dead_code))]
    links: Mutex<HashSet<(u64, u64)>>,
    /// (device, inode) of the folders already queued.
    #[cfg_attr(not(unix), allow(dead_code))]
    dirs: Mutex<HashSet<(u64, u64)>>,
    policy: Policy,
}

/// Which directories the scan is allowed to descend into.
struct Policy {
    #[allow(dead_code)]
    devices: Vec<u64>,
    skip: Vec<PathBuf>,
}

impl Policy {
    fn for_root(root: &Path) -> Policy {
        #[allow(unused_mut)]
        let mut p = Policy { devices: Vec::new(), skip: Vec::new() };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(m) = fs::metadata(root) {
                p.devices.push(m.dev());
            }
            // On macOS the boot volume is split in a read-only system volume and
            // a data volume glued together with firmlinks: `/Users` lives on the
            // data volume, which is also mounted at `/System/Volumes/Data`.
            #[cfg(target_os = "macos")]
            if root == Path::new("/") {
                let data = Path::new("/System/Volumes/Data");
                if let Ok(m) = fs::metadata(data) {
                    p.devices.push(m.dev());
                    p.skip.push(data.to_path_buf());
                }
            }
        }
        #[cfg(not(unix))]
        let _ = root;
        p
    }
}

pub fn start_scan(root: PathBuf) -> ScanHandle {
    let tree = Arc::new(RwLock::new(Tree::new(root.clone())));
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let entries = Arc::new(AtomicU64::new(0));
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let progress = Arc::new(AtomicU64::new(0));
    let started = Instant::now();

    if let Ok(m) = fs::metadata(&root) {
        tree.write().nodes[0].mtime = mtime_of(&m);
    }

    let shared = Arc::new(Shared {
        tree: tree.clone(),
        queue: Mutex::new(vec![(0, root.clone(), None, WHOLE)]),
        cv: Condvar::new(),
        pending: AtomicUsize::new(1),
        cancel: cancel.clone(),
        entries: entries.clone(),
        bytes: bytes.clone(),
        progress: progress.clone(),
        links: Mutex::new(HashSet::new()),
        dirs: Mutex::new(HashSet::new()),
        policy: Policy::for_root(&root),
    });

    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let (done2, elapsed2) = (done.clone(), elapsed_ms.clone());
    std::thread::Builder::new()
        .name("scan-supervisor".into())
        .spawn(move || {
            let handles: Vec<_> = (0..workers)
                .map(|i| {
                    let s = shared.clone();
                    std::thread::Builder::new()
                        .name(format!("scan-{i}"))
                        .spawn(move || worker(&s))
                        .expect("spawn scan worker")
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
            shared.tree.write().generation += 1;
            elapsed2.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
            done2.store(true, Ordering::Release);
        })
        .expect("spawn scan supervisor");

    ScanHandle { tree, entries, started, cancel, done, elapsed_ms, bytes, progress }
}

fn worker(s: &Shared) {
    no_cloud_downloads();
    loop {
        let job = {
            let mut q = s.queue.lock();
            loop {
                if s.cancel.load(Ordering::Acquire) {
                    return;
                }
                if let Some(j) = q.pop() {
                    break j;
                }
                if s.pending.load(Ordering::Acquire) == 0 {
                    return;
                }
                s.cv.wait_for(&mut q, Duration::from_millis(50));
            }
        };
        let handed_on = scan_dir(s, job.0, &job.1, job.2, job.3);
        s.progress.fetch_add(job.3 - handed_on, Ordering::Relaxed);
        if s.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            s.cv.notify_all();
        }
    }
}

/// A folder with more entries than this is reported as partly unread
/// instead of being held in memory whole.
const MAX_FOLDER_ENTRIES: usize = 4_000_000;

/// One folder entry, read without following links.
struct Found {
    name: std::ffi::OsString,
    is_dir: bool,
    is_symlink: bool,
    /// Space allocated on disk.
    size: u64,
    mtime: i64,
    /// Hidden by a file attribute rather than by its name.
    hidden: bool,
    /// (device, inode) where the system has them.
    id: (u64, u64),
    links: u64,
}

/// Reads a folder: its entries and whether all of them could be read. `None`
/// when it cannot be opened, or is no longer the folder that was queued.
#[cfg(unix)]
fn list(s: &Shared, path: &Path, expected: Option<(u64, u64)>) -> Option<(Vec<Found>, bool)> {
    use std::os::unix::ffi::OsStringExt;
    let stop = || s.cancel.load(Ordering::Acquire);
    // The scanned location itself may be a link. Anything below must be the
    // very folder its parent listed: a folder swapped for a link, or for
    // another folder, since then is not read.
    let dir = match expected {
        Some(id) => {
            let dir = Dir::open_nofollow(path).ok()?;
            let st = dir.stat_self().ok()?;
            if (st.dev, st.ino) != id {
                return None;
            }
            dir
        }
        None => Dir::open(path).ok()?,
    };
    let (names, mut complete) = dir.names(MAX_FOLDER_ENTRIES, &stop).ok()?;
    let mut found = Vec::with_capacity(names.len());
    for name in names {
        if found.len() % 1024 == 0 && stop() {
            complete = false;
            break;
        }
        // Relative to the open folder, never through a link.
        let Ok(st) = dir.stat(&name) else {
            complete = false;
            continue;
        };
        found.push(Found {
            name: std::ffi::OsString::from_vec(name.into_bytes()),
            is_dir: st.kind == Kind::Dir,
            is_symlink: st.kind == Kind::Symlink,
            size: st.blocks * 512,
            mtime: st.mtime.max(0),
            hidden: st.hidden,
            id: (st.dev, st.ino),
            links: st.nlink,
        });
    }
    Some((found, complete))
}

#[cfg(not(unix))]
fn list(s: &Shared, path: &Path, _expected: Option<(u64, u64)>) -> Option<(Vec<Found>, bool)> {
    let mut found = Vec::new();
    let mut complete = true;
    for de in fs::read_dir(path).ok()? {
        if found.len() >= MAX_FOLDER_ENTRIES || (found.len() % 1024 == 0 && s.cancel.load(Ordering::Acquire)) {
            complete = false;
            break;
        }
        // `DirEntry::metadata` never follows symlinks.
        let Some((de, md)) = de.ok().and_then(|de| de.metadata().ok().map(|md| (de, md))) else {
            complete = false;
            continue;
        };
        let ft = md.file_type();
        found.push(Found {
            name: de.file_name(),
            is_dir: ft.is_dir(),
            is_symlink: ft.is_symlink(),
            size: disk_size(&md),
            mtime: mtime_of(&md),
            hidden: hidden_attribute(&md),
            id: (0, 0),
            links: 1,
        });
    }
    Some((found, complete))
}

/// Reads one folder into the tree and queues its subfolders. Returns the part
/// of `share` handed on to them.
fn scan_dir(s: &Shared, id: NodeId, path: &Path, expected: Option<(u64, u64)>, share: u64) -> u64 {
    let Some((found, complete)) = list(s, path, expected) else {
        s.tree.write().mark_denied(id);
        return 0;
    };
    let mut entries: Vec<Entry> = Vec::with_capacity(found.len());
    let mut subdirs: Vec<(usize, PathBuf, (u64, u64))> = Vec::new();
    let mut dir_ids: Vec<(usize, (u64, u64))> = Vec::new();
    for f in found {
        let mut flags = 0u8;
        if f.hidden || f.name.to_string_lossy().starts_with('.') {
            flags |= F_HIDDEN;
        }
        let mut size = 0u64;
        if f.is_symlink {
            flags |= F_SYMLINK;
            size = f.size;
        } else if f.is_dir {
            flags |= F_DIR;
            if is_package(&f.name) {
                flags |= F_PACKAGE;
            }
            let child = path.join(&f.name);
            if s.policy.skip.iter().any(|p| p == &child) {
                continue;
            }
            if cfg!(unix) {
                dir_ids.push((entries.len(), f.id));
            }
            if !same_filesystem(&s.policy, f.id.0) {
                flags |= F_OTHER_FS;
            } else if first_visit(s, f.id) {
                // A folder reachable under a second name is read only once.
                subdirs.push((entries.len(), child, f.id));
            }
        } else {
            size = f.size;
            if size > 0 && f.links > 1 && already_counted(s, f.id) {
                size = 0;
            }
        }
        entries.push(Entry { name: f.name, size, mtime: f.mtime, flags });
    }
    s.entries.fetch_add(entries.len() as u64, Ordering::Relaxed);
    s.bytes.fetch_add(entries.iter().map(|e| e.size).sum(), Ordering::Relaxed);
    if !complete {
        // Part of the folder is missing from the results: report it.
        s.tree.write().mark_denied(id);
    }
    if entries.is_empty() {
        return 0;
    }
    let start = {
        let mut tree = s.tree.write();
        let start = tree.add_children(id, entries);
        tree.dir_ids.extend(dir_ids.into_iter().map(|(i, ident)| (start + i as u32, ident)));
        start
    };
    if subdirs.is_empty() {
        return 0;
    }
    let count = subdirs.len() as u64;
    let each = share / count;
    s.pending.fetch_add(subdirs.len(), Ordering::AcqRel);
    let mut q = s.queue.lock();
    for (i, p, identity) in subdirs {
        q.push((start + i as u32, p, cfg!(unix).then_some(identity), each));
    }
    drop(q);
    s.cv.notify_all();
    each * count
}

fn same_filesystem(policy: &Policy, device: u64) -> bool {
    !cfg!(unix) || policy.devices.is_empty() || policy.devices.contains(&device)
}

fn first_visit(s: &Shared, id: (u64, u64)) -> bool {
    !cfg!(unix) || s.dirs.lock().insert(id)
}

/// Counts a multiply linked file only the first time it is met.
fn already_counted(s: &Shared, id: (u64, u64)) -> bool {
    !s.links.lock().insert(id)
}

/// Space actually taken on disk. On Unix the listing reports allocated
/// blocks, so sparse, compressed and cloud-only files are not over-reported.
#[cfg(windows)]
fn disk_size(md: &fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    // Cloud placeholders (OneDrive "online-only") take no local space.
    if md.file_attributes() & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0 {
        return 0;
    }
    md.len()
}


pub(crate) fn mtime_of(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(not(unix))]
fn hidden_attribute(md: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }
    #[cfg(not(windows))]
    {
        let _ = md;
        false
    }
}

/// macOS presents some directories as single documents.
fn is_package(name: &std::ffi::OsStr) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    const EXT: &[&str] = &[
        "app", "bundle", "plugin", "kext", "appex", "photoslibrary", "musiclibrary", "tvlibrary",
        "imovielibrary", "fcpbundle", "logicx", "xcarchive", "prefpane", "saver", "qlgenerator",
    ];
    Path::new(name)
        .extension()
        .map(|e| {
            let e = e.to_string_lossy().to_lowercase();
            EXT.contains(&e.as_str())
        })
        .unwrap_or(false)
}

/// Stops this thread from downloading iCloud "dataless" files and folders
/// just because they are being listed.
fn no_cloud_downloads() {
    #[cfg(target_os = "macos")]
    unsafe {
        extern "C" {
            fn setiopolicy_np(iotype: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
        }
        const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: libc::c_int = 3;
        const IOPOL_SCOPE_THREAD: libc::c_int = 1;
        const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: libc::c_int = 1;
        setiopolicy_np(
            IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
            IOPOL_SCOPE_THREAD,
            IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wait(h: &ScanHandle) {
        let t = Instant::now();
        while !h.is_done() {
            assert!(t.elapsed() < Duration::from_secs(20), "scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn write(p: &Path, bytes: usize) {
        let mut f = fs::File::create(p).unwrap();
        f.write_all(&vec![7u8; bytes]).unwrap();
        f.sync_all().unwrap();
    }

    #[test]
    fn progress_ends_on_the_whole() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for sub in ["a/b/c", "a/d", "e", "f/g/h/i", "f/j"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        for i in 0..7 {
            fs::create_dir_all(root.join(format!("a/b/n{i}/m"))).unwrap();
        }
        write(&root.join("a/b/c/file"), 5000);
        let h = start_scan(root.to_path_buf());
        let (mut last, t) = (0.0, Instant::now());
        while !h.is_done() {
            assert!(t.elapsed() < Duration::from_secs(20), "scan timed out");
            let now = h.progress();
            assert!((last..=1.0).contains(&now), "progress went from {last} to {now}");
            last = now;
        }
        // Shares that do not divide evenly stay with the folder that split them.
        assert_eq!(h.progress.load(Ordering::Relaxed), WHOLE);
        assert!(h.bytes() >= 5000);
    }

    #[test]
    fn scans_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        write(&root.join("a/big.bin"), 300_000);
        write(&root.join("a/b/mid.bin"), 100_000);
        write(&root.join("a/b/c/small.txt"), 10);
        write(&root.join(".hidden/x"), 5000);
        write(&root.join("top.txt"), 20_000);

        let h = start_scan(root.to_path_buf());
        wait(&h);
        let t = h.tree.read();
        assert_eq!(t.node(0).items, 5);
        assert!(t.node(0).size >= 425_010, "size {}", t.node(0).size);
        let top = t.sorted_children(0);
        assert_eq!(t.node(top[0]).display_name(), "a");
        let hidden = top.iter().find(|&&c| t.node(c).display_name() == ".hidden").unwrap();
        assert!(t.node(*hidden).is_hidden());
        let big = t.biggest_files(1)[0];
        assert_eq!(t.path(big), root.join("a/big.bin"));
        assert!(t.node(big).mtime > 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed_and_hard_links_count_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("real")).unwrap();
        write(&root.join("real/data.bin"), 200_000);
        std::os::unix::fs::symlink(root.join("real"), root.join("loop")).unwrap();
        std::os::unix::fs::symlink(root, root.join("real/up")).unwrap();
        fs::hard_link(root.join("real/data.bin"), root.join("twin.bin")).unwrap();

        let h = start_scan(root.to_path_buf());
        wait(&h);
        let t = h.tree.read();
        assert_eq!(t.node(0).items, 4);
        let total = t.node(0).size;
        assert!((200_000..400_000).contains(&total), "hard link counted twice: {total}");
    }

    #[test]
    fn cancel_stops_early() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            fs::create_dir_all(dir.path().join(format!("d{i}/e/f"))).unwrap();
        }
        let h = start_scan(dir.path().to_path_buf());
        h.cancel();
        wait(&h);
        assert!(h.was_cancelled());
    }

    #[test]
    fn unreadable_root_is_reported() {
        let h = start_scan(PathBuf::from("/definitely/not/here/xyz"));
        wait(&h);
        assert_eq!(h.tree.read().denied_dirs, 1);
    }
}
