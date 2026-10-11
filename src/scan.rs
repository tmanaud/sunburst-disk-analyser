//! Parallel directory scanner.
//!
//! Worker threads pull directories from a shared stack, read them and append
//! their entries to the shared [`Tree`] one directory at a time, so the UI can
//! show results while the scan is still running.

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
    /// A handle around an already complete tree (demo data, tests).
    pub fn finished(tree: Tree) -> Self {
        ScanHandle {
            tree: Arc::new(RwLock::new(tree)),
            entries: Arc::new(AtomicU64::new(0)),
            started: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(true)),
            elapsed_ms: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct Shared {
    tree: Arc<RwLock<Tree>>,
    queue: Mutex<Vec<(NodeId, PathBuf)>>,
    cv: Condvar,
    /// Directories queued or being processed.
    pending: AtomicUsize,
    cancel: Arc<AtomicBool>,
    entries: Arc<AtomicU64>,
    /// (device, inode) of multiply linked files already counted.
    #[cfg_attr(not(unix), allow(dead_code))]
    links: Mutex<HashSet<(u64, u64)>>,
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
    let started = Instant::now();

    if let Ok(m) = fs::metadata(&root) {
        tree.write().nodes[0].mtime = mtime_of(&m);
    }

    let shared = Arc::new(Shared {
        tree: tree.clone(),
        queue: Mutex::new(vec![(0, root.clone())]),
        cv: Condvar::new(),
        pending: AtomicUsize::new(1),
        cancel: cancel.clone(),
        entries: entries.clone(),
        links: Mutex::new(HashSet::new()),
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

    ScanHandle { tree, entries, started, cancel, done, elapsed_ms }
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
        scan_dir(s, job.0, &job.1);
        if s.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            s.cv.notify_all();
        }
    }
}

fn scan_dir(s: &Shared, id: NodeId, path: &Path) {
    let rd = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(_) => {
            s.tree.write().mark_denied(id);
            return;
        }
    };
    let mut entries: Vec<Entry> = Vec::new();
    let mut subdirs: Vec<(usize, PathBuf)> = Vec::new();
    let mut unreadable = false;
    for de in rd {
        let Ok(de) = de else {
            unreadable = true;
            continue;
        };
        // `DirEntry::metadata` never follows symlinks.
        let Ok(md) = de.metadata() else {
            unreadable = true;
            continue;
        };
        let name = de.file_name();
        let ft = md.file_type();
        let mut flags = 0u8;
        if is_hidden(&name, &md) {
            flags |= F_HIDDEN;
        }
        let mut size = 0u64;
        if ft.is_symlink() {
            flags |= F_SYMLINK;
            size = disk_size(&md);
        } else if ft.is_dir() {
            flags |= F_DIR;
            if is_package(&name) {
                flags |= F_PACKAGE;
            }
            let child = path.join(&name);
            if s.policy.skip.iter().any(|p| p == &child) {
                continue;
            }
            if same_filesystem(&s.policy, &md) {
                subdirs.push((entries.len(), child));
            } else {
                flags |= F_OTHER_FS;
            }
        } else {
            size = disk_size(&md);
            if size > 0 && already_counted(s, &md) {
                size = 0;
            }
        }
        entries.push(Entry { name, size, mtime: mtime_of(&md), flags });
    }
    s.entries.fetch_add(entries.len() as u64, Ordering::Relaxed);
    if unreadable {
        // Part of the folder is missing from the results: report it.
        s.tree.write().mark_denied(id);
    }
    if entries.is_empty() {
        return;
    }
    let start = s.tree.write().add_children(id, entries);
    if !subdirs.is_empty() {
        s.pending.fetch_add(subdirs.len(), Ordering::AcqRel);
        let mut q = s.queue.lock();
        for (i, p) in subdirs {
            q.push((start + i as u32, p));
        }
        drop(q);
        s.cv.notify_all();
    }
}

#[cfg(unix)]
fn same_filesystem(policy: &Policy, md: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    policy.devices.is_empty() || policy.devices.contains(&md.dev())
}

#[cfg(not(unix))]
fn same_filesystem(_policy: &Policy, _md: &fs::Metadata) -> bool {
    true
}

/// Counts a multiply linked file only the first time it is met.
#[cfg(unix)]
fn already_counted(s: &Shared, md: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    md.nlink() > 1 && !s.links.lock().insert((md.dev(), md.ino()))
}

#[cfg(not(unix))]
fn already_counted(_s: &Shared, _md: &fs::Metadata) -> bool {
    false
}

/// Space actually allocated on disk, so sparse, compressed and cloud-only
/// files are not over-reported.
#[cfg(unix)]
fn disk_size(md: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.blocks() * 512
}

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

#[cfg(not(any(unix, windows)))]
fn disk_size(md: &fs::Metadata) -> u64 {
    md.len()
}

fn mtime_of(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_hidden(name: &std::ffi::OsStr, md: &fs::Metadata) -> bool {
    if name.to_string_lossy().starts_with('.') {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const UF_HIDDEN: u32 = 0x8000;
        if md.st_flags() & UF_HIDDEN != 0 {
            return true;
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
            return true;
        }
    }
    let _ = md;
    false
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
