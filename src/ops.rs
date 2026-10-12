//! Remove (to the system Trash), move and copy of selected items.
//!
//! Nothing here ever overwrites an existing file, deletes permanently or
//! falls back to copy-then-delete: a remove or a move is a rename, or fails.

use crate::tree::NodeId;
use parking_lot::Mutex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

#[cfg(unix)]
use crate::pinned::{cstr, set_mode, Dir, Kind, Stat};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    Remove,
    Move,
    Copy,
}

#[derive(Clone)]
pub struct OpItem {
    pub id: NodeId,
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    /// Identity recorded when the item was put up for review; none when it
    /// no longer matched the scan, in which case it is not touched.
    pub ident: Option<Ident>,
}

pub struct OpResult {
    pub item: OpItem,
    pub error: Option<String>,
    /// Final location when the item had to be renamed to avoid a collision.
    pub renamed: Option<PathBuf>,
}

pub struct OpProgress {
    pub kind: OpKind,
    pub total: usize,
    pub done: AtomicUsize,
    pub finished: AtomicBool,
    pub cancel: AtomicBool,
    pub results: Mutex<Vec<OpResult>>,
}

/// Why an item must not be removed or moved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protected {
    /// A volume root, the home folder or an operating system folder.
    System,
}

/// Folds case on the file systems that ignore it by default.
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Removes the `\\?\` prefix Windows adds to canonical paths.
pub fn plain(path: &Path) -> PathBuf {
    if cfg!(windows) {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return PathBuf::from(rest.to_string());
            }
        }
    }
    path.to_path_buf()
}

/// The item's real location: links in the parent folders are resolved, the
/// item itself is not (a link is removed as a link).
fn resolved(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match fs::canonicalize(parent) {
            Ok(real) => plain(&real).join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

pub fn protected_reason(path: &Path) -> Option<Protected> {
    let real = resolved(path);
    if protected_at(&plain(path)).is_some() || protected_at(&real).is_some() {
        return Some(Protected::System);
    }
    None
}

fn protected_at(path: &Path) -> Option<Protected> {
    if path.parent().is_none() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Some(Protected::System);
    }
    if let Some(home) = dirs::home_dir() {
        let user_dirs = [
            Some(home.clone()),
            home.parent().map(Path::to_path_buf),
            dirs::desktop_dir(),
            dirs::document_dir(),
            dirs::download_dir(),
            dirs::picture_dir(),
            dirs::audio_dir(),
            dirs::video_dir(),
            Some(home.join("Library")),
        ];
        if user_dirs.iter().flatten().any(|d| same_path(d, path)) {
            return Some(Protected::System);
        }
    }
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    // Folders that are protected themselves but whose content is the user's.
    let mut exact: Vec<PathBuf> = Vec::new();
    // Folders protected together with everything inside them.
    let mut trees: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        exact.extend(["/Applications", "/Users", "/Volumes", "/Library", "/opt", "/Users/Shared", "/private/tmp", "/tmp"].map(PathBuf::from));
        trees.extend(["/System", "/usr", "/bin", "/sbin", "/private", "/etc", "/var", "/dev", "/cores"].map(PathBuf::from));
    } else if cfg!(windows) {
        // Windows is not always installed on C:.
        exact.extend(["ProgramFiles", "ProgramFiles(x86)", "ProgramData", "PUBLIC"].iter().filter_map(|k| var(k)));
        trees.extend(var("SystemRoot").or_else(|| Some(PathBuf::from("C:\\Windows"))));
    } else {
        exact.extend(["/home", "/opt", "/mnt", "/media", "/srv", "/tmp", "/var", "/root"].map(PathBuf::from));
        trees.extend(["/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/run", "/sbin", "/sys", "/usr"].map(PathBuf::from));
    }
    if exact.iter().any(|e| same_path(e, path)) {
        return Some(Protected::System);
    }
    if trees.iter().any(|t| path.ancestors().any(|a| same_path(a, t))) {
        // Temporary folders live under /private on macOS and stay usable:
        // the shared /tmp and this user's own temporary folder.
        let temp = std::env::temp_dir();
        let real_temp = fs::canonicalize(&temp).unwrap_or_else(|_| temp.clone());
        let in_temp = cfg!(target_os = "macos")
            && [Path::new("/private/tmp"), Path::new("/tmp"), temp.as_path(), real_temp.as_path()]
                .iter()
                .any(|tmp| path.starts_with(tmp) && !same_path(path, tmp));
        if !in_temp {
            return Some(Protected::System);
        }
    }
    None
}

/// Runs the operation on a background thread.
pub fn run(kind: OpKind, items: Vec<OpItem>, dest: Option<PathBuf>) -> Arc<OpProgress> {
    let progress = Arc::new(OpProgress {
        kind,
        total: items.len(),
        done: AtomicUsize::new(0),
        finished: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
        results: Mutex::new(Vec::new()),
    });
    let p = progress.clone();
    std::thread::Builder::new()
        .name("file-ops".into())
        .spawn(move || {
            for item in items {
                if p.cancel.load(Ordering::Acquire) {
                    break;
                }
                let outcome = run_one(kind, &item, dest.as_deref(), &p.cancel);
                let (error, renamed) = match outcome {
                    Ok(r) => (None, r),
                    Err(e) => (Some(e), None),
                };
                p.results.lock().push(OpResult { item, error, renamed });
                p.done.fetch_add(1, Ordering::AcqRel);
            }
            p.finished.store(true, Ordering::Release);
        })
        .expect("spawn file operation thread");
    progress
}

fn run_one(kind: OpKind, item: &OpItem, dest: Option<&Path>, cancel: &AtomicBool) -> Result<Option<PathBuf>, String> {
    // Without a recorded identity nothing proves this is the reviewed item.
    let reviewed = match item.ident {
        Some(ident) => ident,
        None if fs::symlink_metadata(&item.path).is_err() => return Err("The item no longer exists".into()),
        None => return Err("The item changed since it was scanned: scan the folder again".into()),
    };
    if reviewed.is_dir != item.is_dir {
        return Err("The item changed since it was reviewed".into());
    }
    if kind != OpKind::Copy && protected_reason(&item.path).is_some() {
        return Err("This item is required by the system".into());
    }
    if kind == OpKind::Remove {
        // Time has passed since the review: the item must still be the same object.
        move_to_trash(&Source::open(item, reviewed)?)?;
        return Ok(None);
    }
    let dest = dest.ok_or("No destination folder")?;
    if kind == OpKind::Move && item.path.parent() == Some(dest) {
        return Err("The item is already in this folder".into());
    }
    check_destination(&item.path, dest)?;
    let src = Source::open(item, reviewed)?;
    // A move is a rename and nothing else: across volumes it would mean
    // copy-then-delete, which cannot be made safe against files changing
    // meanwhile, so that case is refused.
    let (target, renamed) = if kind == OpKind::Move { src.move_into(dest)? } else { src.copy_into(dest, cancel)? };
    Ok(renamed.then_some(target))
}

/// What identifies an item between the review and the operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ident {
    pub is_dir: bool,
    /// Volume and file number, then length and modification time for a
    /// file. A folder in use changes date all the time and stays the same
    /// folder, so only what it is counts.
    key: [u64; 4],
}

impl Ident {
    fn new(is_dir: bool, volume: u64, file: u64, len: u64, mtime: u64) -> Ident {
        Ident { is_dir, key: if is_dir { [volume, file, 0, 0] } else { [volume, file, len, mtime] } }
    }

    pub fn of(path: &Path) -> Option<Ident> {
        let md = fs::symlink_metadata(path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Ident::new(md.is_dir(), md.dev(), md.ino(), md.len(), nanos(md.mtime(), md.mtime_nsec())))
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use std::os::windows::io::AsRawHandle;
            extern "system" {
                fn GetFileInformationByHandle(file: *mut std::ffi::c_void, info: *mut [u32; 13]) -> i32;
            }
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            // No access is requested: only the handle's identity is read.
            let file = fs::OpenOptions::new()
                .access_mode(0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)
                .ok()?;
            // BY_HANDLE_FILE_INFORMATION: attributes, three times, volume
            // serial number, size, link count, file index.
            let mut info = [0u32; 13];
            if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0 {
                return None;
            }
            let wide = |high: u32, low: u32| (high as u64) << 32 | low as u64;
            Some(Ident::new(md.is_dir(), info[7] as u64, wide(info[11], info[12]), md.len(), wide(info[6], info[5])))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64);
            Some(Ident::new(md.is_dir(), 0, 0, md.len(), mtime.unwrap_or(0)))
        }
    }

    #[cfg(unix)]
    fn of_stat(st: &Stat) -> Ident {
        Ident::new(st.kind == Kind::Dir, st.dev, st.ino, st.len, nanos(st.mtime, st.mtime_nsec))
    }
}

#[cfg(unix)]
fn nanos(secs: i64, nsec: i64) -> u64 {
    (secs as u64).wrapping_mul(1_000_000_000).wrapping_add(nsec as u64)
}

/// Whether the item on disk is still the one the scan recorded: a file with
/// the same date, or the same folder (`dir_id` is its scan-time device and
/// inode, where known).
pub fn unchanged_since_scan(path: &Path, is_dir: bool, mtime: i64, dir_id: Option<(u64, u64)>) -> bool {
    let Ok(md) = fs::symlink_metadata(path) else { return false };
    if md.is_dir() != is_dir {
        return false;
    }
    if !is_dir {
        return crate::scan::mtime_of(&md) == mtime;
    }
    #[cfg(unix)]
    if let Some(id) = dir_id {
        use std::os::unix::fs::MetadataExt;
        return (md.dev(), md.ino()) == id;
    }
    let _ = dir_id;
    true
}

fn check_destination(src: &Path, dest: &Path) -> Result<(), String> {
    if !dest.is_dir() {
        return Err("The destination folder does not exist".into());
    }
    if let (Ok(s), Ok(d)) = (fs::canonicalize(src), fs::canonicalize(dest)) {
        if d.starts_with(&s) {
            return Err("The destination is inside the item".into());
        }
    }
    Ok(())
}

/// The n-th candidate name for `src` in `dest`: "name", "name (2)", "name (3)"…
fn candidate(src: &Path, dest: &Path, n: u32) -> Result<PathBuf, String> {
    let name = src.file_name().ok_or("Invalid item name")?;
    if n <= 1 {
        return Ok(dest.join(name));
    }
    let p = Path::new(name);
    let stem = p.file_stem().unwrap_or(name).to_string_lossy().into_owned();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    Ok(dest.join(format!("{stem} ({n}){ext}")))
}

const CROSS_VOLUME: &str = "The destination is on another volume: copy the item, check the copy, then remove the original";

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Stopped before the item was fully copied")
}

/// Copies the content of one file, giving up as soon as Stop is pressed.
fn copy_bytes(input: &mut impl io::Read, output: &mut impl io::Write, cancel: &AtomicBool) -> io::Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(stopped());
        }
        match input.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => output.write_all(&buf[..n])?,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

// ---- macOS and Linux: every access goes through directory handles ---------

/// The reviewed item, pinned: its folder is open and the entry was checked
/// against the reviewed identity through that handle.
#[cfg(unix)]
struct Source<'a> {
    path: &'a Path,
    dir: Dir,
    name: std::ffi::CString,
    stat: Stat,
}

#[cfg(unix)]
impl<'a> Source<'a> {
    fn open(item: &'a OpItem, reviewed: Ident) -> Result<Self, String> {
        let gone = |_| String::from("The item no longer exists");
        let (parent, name) = item.path.parent().zip(item.path.file_name()).ok_or("Invalid item name")?;
        let dir = Dir::open(parent).map_err(gone)?;
        let name = cstr(name).map_err(|e| e.to_string())?;
        let stat = dir.stat(&name).map_err(gone)?;
        if Ident::of_stat(&stat) != reviewed {
            return Err("The item changed since it was reviewed".into());
        }
        // The scan does not look inside other volumes, so their content was
        // never reviewed.
        if stat.kind == Kind::Dir && dir.stat_self().map(|d| d.dev != stat.dev).unwrap_or(true) {
            return Err("This item is another volume and cannot be handled from here".into());
        }
        Ok(Source { path: &item.path, dir, name, stat })
    }

    fn move_into(&self, dest: &Path) -> Result<(PathBuf, bool), String> {
        publish(&self.dir, &self.name, &open_destination(dest)?, self.path, dest)
    }

    fn copy_into(&self, dest: &Path, cancel: &AtomicBool) -> Result<(PathBuf, bool), String> {
        let to = open_destination(dest)?;
        raise_open_files_limit();
        let staging = Staging::create(&to).map_err(|e| e.to_string())?;
        let result = copy_tree(self, &staging, cancel)
            .map_err(|e| e.to_string())
            .and_then(|()| publish(&staging.dir, &self.name, &to, self.path, dest));
        staging.discard(&to);
        result
    }
}

#[cfg(unix)]
fn open_destination(dest: &Path) -> Result<Dir, String> {
    Dir::open(dest).map_err(|_| "The destination folder does not exist".into())
}

/// A copy keeps one handle per folder level open.
#[cfg(unix)]
fn raise_open_files_limit() {
    unsafe {
        let mut limit = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
            let wanted = limit.rlim_max.min(8192);
            if limit.rlim_cur < wanted {
                limit.rlim_cur = wanted;
                libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
            }
        }
    }
}

/// Private folder in the destination where a copy is built before it is
/// renamed to its final name, so nobody else can read it meanwhile.
#[cfg(unix)]
struct Staging {
    name: std::ffi::CString,
    dir: Dir,
    id: (u64, u64),
}

#[cfg(unix)]
impl Staging {
    fn create(dest: &Dir) -> io::Result<Staging> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let name = std::ffi::CString::new(format!(".sunburst-copy-{}-{nanos}", std::process::id())).expect("no NUL");
        dest.create_dir(&name)?;
        let dir = dest.open_dir(&name)?;
        let st = dir.stat_self()?;
        // Where others may write to the destination, the folder that was just
        // opened must be the one created above and not somebody else's.
        let shared = dest.stat_self()?.mode & 0o022 != 0;
        if shared && st.uid != unsafe { libc::geteuid() } {
            return Err(io::Error::other("The destination folder changed during the copy"));
        }
        Ok(Staging { name, dir, id: (st.dev, st.ino) })
    }

    /// Deletes whatever was not published, through the handle: only objects
    /// created by this copy can be reached that way.
    fn discard(self, dest: &Dir) {
        let _ = self.dir.clear();
        let _ = dest.unlink(&self.name, true);
    }
}

#[cfg(unix)]
fn publish(from: &Dir, name: &std::ffi::CStr, to: &Dir, named_like: &Path, dest: &Path) -> Result<(PathBuf, bool), String> {
    for n in 1..10_000 {
        let target = candidate(named_like, dest, n)?;
        let target_name = target.file_name().map(cstr).ok_or("Invalid item name")?.map_err(|e| e.to_string())?;
        match from.rename_excl(name, to, &target_name) {
            Ok(()) => return Ok((target, n > 1)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => return Err(CROSS_VOLUME.into()),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not find a free name in the destination".into())
}

/// Copies the item into the staging folder. Folders are walked with an
/// explicit stack, entries are opened relative to their folder's handle and
/// never through a link, and the copy stays on the item's volume.
#[cfg(unix)]
fn copy_tree(src: &Source, staging: &Staging, cancel: &AtomicBool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    struct Frame {
        src: Dir,
        dst: Dir,
        names: Vec<std::ffi::CString>,
        mode: u32,
    }
    let changed = || io::Error::other("An item changed while it was being copied");
    let entry = |from: &Dir, to: &Dir, name: &std::ffi::CStr, st: Stat| -> io::Result<Option<Frame>> {
        let shown = || name.to_string_lossy().into_owned();
        match st.kind {
            Kind::Symlink => to.symlink(&from.read_link(name)?, name).map(|()| None),
            Kind::Dir => {
                if st.dev != src.stat.dev {
                    return Err(io::Error::other(format!("{} is another volume, which the scan did not look into", shown())));
                }
                if (st.dev, st.ino) == staging.id {
                    return Err(io::Error::other("The destination is inside the item"));
                }
                let from = from.open_dir(name)?;
                let opened = from.stat_self()?;
                if (opened.dev, opened.ino) != (st.dev, st.ino) {
                    return Err(changed());
                }
                to.create_dir(name)?;
                let to = to.open_dir(name)?;
                let (names, complete) = from.names(usize::MAX, &|| cancel.load(Ordering::Acquire))?;
                if !complete {
                    return Err(if cancel.load(Ordering::Acquire) { stopped() } else { changed() });
                }
                Ok(Some(Frame { src: from, dst: to, names, mode: st.mode }))
            }
            Kind::File => {
                let mut input = from.open_file(name)?;
                let opened = Stat::of_file(&input)?;
                if opened.kind != Kind::File || (opened.dev, opened.ino) != (st.dev, st.ino) {
                    return Err(changed());
                }
                let mut output = to.create_file(name)?;
                copy_bytes(&mut input, &mut output, cancel)?;
                set_mode(output.as_raw_fd(), st.mode)?;
                if let Ok(t) = input.metadata().and_then(|m| m.modified()) {
                    let _ = output.set_modified(t);
                }
                Ok(None)
            }
            // Sockets, devices and pipes cannot be copied.
            Kind::Other => Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} is a special file", shown()))),
        }
    };
    let mut stack: Vec<Frame> = Vec::new();
    stack.extend(entry(&src.dir, &staging.dir, &src.name, src.stat)?);
    while let Some(top) = stack.last_mut() {
        if cancel.load(Ordering::Acquire) {
            return Err(stopped());
        }
        match top.names.pop() {
            Some(name) => {
                let st = top.src.stat(&name)?;
                let sub = entry(&top.src, &top.dst, &name, st)?;
                stack.extend(sub);
            }
            None => {
                let done = stack.pop().expect("frame");
                set_mode(done.dst.as_raw_fd(), done.mode)?;
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn move_to_trash(src: &Source) -> Result<(), String> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut ctx = trash::TrashContext::default();
    // The default method scripts Finder, which needs an Automation permission
    // prompt; NSFileManager trashes directly.
    ctx.set_delete_method(DeleteMethod::NsFileManager);
    ctx.delete(src.path).map_err(|e| e.to_string())
}

/// Trashing is a rename into a trash folder of the item's own volume. Where
/// no such folder can be used the item stays in place: copying it to another
/// volume and deleting the original would not be a safe substitute.
#[cfg(all(unix, not(target_os = "macos")))]
fn move_to_trash(src: &Source) -> Result<(), String> {
    let gone = "The item no longer exists";
    let parent = src.path.parent().and_then(|p| fs::canonicalize(p).ok()).ok_or(gone)?;
    let real = parent.join(src.path.file_name().ok_or(gone)?);
    let device = src.dir.stat_self().map_err(|e| e.to_string())?.dev;
    let mut why = String::from("none found");
    for (trash, top) in xdg_trash::candidates(&parent, device) {
        let recorded = top.as_deref().and_then(|t| real.strip_prefix(t).ok()).unwrap_or(&real);
        match xdg_trash::put(&src.dir, &src.name, &trash, recorded) {
            Ok(()) => return Ok(()),
            Err(e) => why = e.to_string(),
        }
    }
    Err(format!("This volume has no usable Trash, so the item was left in place ({why})"))
}

/// FreeDesktop.org trash folders, written to with renames only.
#[cfg(all(unix, any(not(target_os = "macos"), test)))]
mod xdg_trash {
    use crate::pinned::{cstr, Dir};
    use std::ffi::{CStr, OsStr};
    use std::io::{self, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::path::{Path, PathBuf};

    /// Trash folders that may take an item living in `parent` (a resolved
    /// path on `device`), best first, each with the volume root its recorded
    /// paths are relative to (none for the home trash).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn candidates(parent: &Path, device: u64) -> Vec<(PathBuf, Option<PathBuf>)> {
        let uid = unsafe { libc::geteuid() };
        let mut found = Vec::new();
        if let Some(data) = dirs::data_dir() {
            if data.ancestors().find_map(|a| std::fs::metadata(a).ok()).map(|m| m.dev()) == Some(device) {
                found.push((data.join("Trash"), None));
            }
        }
        let mut top = parent;
        for ancestor in parent.ancestors().skip(1) {
            match std::fs::symlink_metadata(ancestor) {
                Ok(m) if m.dev() == device => top = ancestor,
                _ => break,
            }
        }
        // A shared `.Trash` must be a real folder with the sticky bit.
        let shared = top.join(".Trash");
        if std::fs::symlink_metadata(&shared).map(|m| m.is_dir() && m.mode() & 0o1000 != 0).unwrap_or(false) {
            found.push((shared.join(uid.to_string()), Some(top.to_path_buf())));
        }
        found.push((top.join(format!(".Trash-{uid}")), Some(top.to_path_buf())));
        found
    }

    /// Moves `name` out of `src` into the trash folder `trash`. `recorded`
    /// is the original location kept for restoring.
    pub fn put(src: &Dir, name: &CStr, trash: &Path, recorded: &Path) -> io::Result<()> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(trash.join("files"))?;
        builder.create(trash.join("info"))?;
        let files = Dir::open_nofollow(&trash.join("files"))?;
        let info = Dir::open_nofollow(&trash.join("info"))?;
        let original = Path::new(OsStr::from_bytes(name.to_bytes()));
        for n in 1..10_000 {
            let kept = super::candidate(original, Path::new(""), n).map_err(io::Error::other)?.into_os_string();
            let mut info_name = kept.clone();
            info_name.push(".trashinfo");
            let (kept, info_name) = (cstr(&kept)?, cstr(&info_name)?);
            // The info file comes first and reserves the name.
            let mut file = match info.create_file(&info_name) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            };
            let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
            let moved = write!(file, "[Trash Info]\nPath={}\nDeletionDate={date}\n", encode(recorded))
                .and_then(|()| src.rename_excl(name, &files, &kept));
            match moved {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let _ = info.unlink(&info_name, false);
                    if e.kind() != io::ErrorKind::AlreadyExists {
                        return Err(e);
                    }
                }
            }
        }
        Err(io::Error::other("Could not find a free name in the Trash"))
    }

    fn encode(path: &Path) -> String {
        let mut out = String::new();
        for &b in path.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }
}

// ---- other systems: paths, checked right before use -----------------------

#[cfg(not(unix))]
struct Source<'a> {
    path: &'a Path,
}

#[cfg(not(unix))]
impl<'a> Source<'a> {
    fn open(item: &'a OpItem, reviewed: Ident) -> Result<Self, String> {
        let now = Ident::of(&item.path).ok_or("The item no longer exists")?;
        if now != reviewed {
            return Err("The item changed since it was reviewed".into());
        }
        Ok(Source { path: &item.path })
    }

    fn move_into(&self, dest: &Path) -> Result<(PathBuf, bool), String> {
        publish(self.path, self.path, dest)
    }

    fn copy_into(&self, dest: &Path, cancel: &AtomicBool) -> Result<(PathBuf, bool), String> {
        let name = self.path.file_name().ok_or("Invalid item name")?;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        // A name in `dest` that no other program will plausibly use.
        let mut staged = std::ffi::OsString::from(format!(".sunburst-copy-{}-{nanos}-", std::process::id()));
        staged.push(name);
        let staging = dest.join(staged);
        // `owned` tells whether the staging object was created by this copy;
        // nothing else is ever cleaned up.
        let mut owned = false;
        let result = copy_any(self.path, &staging, &mut owned, cancel).map_err(|e| e.to_string()).and_then(|()| publish(&staging, self.path, dest));
        if result.is_err() && owned {
            let _ = remove_any(&staging);
        }
        result
    }
}

/// Renames `from` to a free name in `dest` without ever replacing anything.
/// Returns the final path and whether a numbered name had to be used.
#[cfg(not(unix))]
fn publish(from: &Path, named_like: &Path, dest: &Path) -> Result<(PathBuf, bool), String> {
    for n in 1..10_000 {
        let target = candidate(named_like, dest, n)?;
        match rename_noreplace(from, &target) {
            Ok(()) => return Ok((target, n > 1)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // ERROR_NOT_SAME_DEVICE
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(17) => return Err(CROSS_VOLUME.into()),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not find a free name in the destination".into())
}

/// Atomic rename that fails with `AlreadyExists` instead of replacing `to`.
#[cfg(windows)]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() };
    let (wf, wt) = (wide(from), wide(to));
    // No MOVEFILE_REPLACE_EXISTING and no MOVEFILE_COPY_ALLOWED.
    if unsafe { MoveFileExW(wf.as_ptr(), wt.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::rename(from, to),
        Err(e) => Err(e),
        Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
    }
}

/// Copies `src` to `dst`, which must not exist. `owned` is set as soon as the
/// top-level object has been created by this call.
#[cfg(not(unix))]
fn copy_any(src: &Path, dst: &Path, owned: &mut bool, cancel: &AtomicBool) -> io::Result<()> {
    // Folders being copied: what is left to read and the permissions to set
    // once the folder is complete.
    let mut stack: Vec<(fs::ReadDir, PathBuf, fs::Permissions)> = Vec::new();
    let mut next = Some((src.to_path_buf(), dst.to_path_buf()));
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(stopped());
        }
        if let Some((from, to)) = next.take() {
            let md = fs::symlink_metadata(&from)?;
            let ft = md.file_type();
            if ft.is_symlink() {
                copy_symlink(&from, &to)?;
                *owned = true;
            } else if ft.is_dir() {
                fs::create_dir(&to)?;
                *owned = true;
                stack.push((fs::read_dir(&from)?, to, md.permissions()));
            } else if ft.is_file() {
                // `create_new` guarantees an existing file is never overwritten.
                let mut output = fs::OpenOptions::new().write(true).create_new(true).open(&to)?;
                *owned = true;
                let mut input = fs::File::open(&from)?;
                copy_bytes(&mut input, &mut output, cancel)?;
                let _ = fs::set_permissions(&to, md.permissions());
                if let Ok(t) = md.modified() {
                    let _ = output.set_modified(t);
                }
            } else {
                return Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} is a special file", from.display())));
            }
        }
        let Some((entries, to, permissions)) = stack.last_mut() else { return Ok(()) };
        match entries.next() {
            Some(entry) => {
                let entry = entry?;
                next = Some((entry.path(), to.join(entry.file_name())));
            }
            None => {
                let _ = fs::set_permissions(&*to, permissions.clone());
                stack.pop();
            }
        }
    }
}

#[cfg(windows)]
fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    if fs::metadata(src).map(|m| m.is_dir()).unwrap_or(false) {
        std::os::windows::fs::symlink_dir(target, dst)
    } else {
        std::os::windows::fs::symlink_file(target, dst)
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_symlink(src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} is a link", src.display())))
}

/// Removes a staging object this module created itself.
#[cfg(not(unix))]
fn remove_any(p: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// Whether Windows has a Recycle Bin for the item: only internal drives do.
/// Elsewhere (network shares, memory sticks) it deletes for good.
#[cfg(windows)]
fn has_recycle_bin(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_FIXED: u32 = 3;
    let real = resolved(path);
    let Some(root) = real.ancestors().last() else { return false };
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    if wide.last() != Some(&(b'\\' as u16)) {
        wide.push(b'\\' as u16);
    }
    wide.push(0);
    unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_FIXED }
}

#[cfg(not(unix))]
fn move_to_trash(src: &Source) -> Result<(), String> {
    #[cfg(windows)]
    if !has_recycle_bin(src.path) {
        return Err("This drive has no Recycle Bin: the item would be deleted for good, so it was left in place".into());
    }
    trash::TrashContext::default().delete(src.path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(p: &OpProgress) {
        let t = std::time::Instant::now();
        while !p.finished.load(Ordering::Acquire) {
            assert!(t.elapsed().as_secs() < 20);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn item(path: PathBuf) -> OpItem {
        let is_dir = path.is_dir();
        let ident = Ident::of(&path);
        OpItem { id: 0, path, size: 0, is_dir, ident }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(src.join("folder/sub")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("folder/a.txt"), b"alpha").unwrap();
        fs::write(src.join("folder/sub/b.txt"), b"beta").unwrap();
        fs::write(src.join("file.txt"), b"one").unwrap();
        (dir, src, dst)
    }

    #[test]
    fn copy_keeps_source_and_renames_on_collision() {
        let (_d, src, dst) = fixture();
        fs::write(dst.join("file.txt"), b"existing").unwrap();
        let p = run(OpKind::Copy, vec![item(src.join("file.txt")), item(src.join("folder"))], Some(dst.clone()));
        wait(&p);
        let r = p.results.lock();
        assert!(r.iter().all(|x| x.error.is_none()));
        assert_eq!(r[0].renamed.as_deref(), Some(dst.join("file (2).txt").as_path()));
        assert_eq!(fs::read(dst.join("file.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(dst.join("file (2).txt")).unwrap(), b"one");
        assert_eq!(fs::read(dst.join("folder/sub/b.txt")).unwrap(), b"beta");
        assert!(src.join("folder/a.txt").exists());
    }

    #[test]
    fn move_never_replaces_and_identity_is_checked() {
        let (_d, src, dst) = fixture();
        fs::create_dir(dst.join("folder")).unwrap();
        fs::write(dst.join("folder/keep.txt"), b"keep").unwrap();
        let p = run(OpKind::Move, vec![item(src.join("folder"))], Some(dst.clone()));
        wait(&p);
        assert_eq!(p.results.lock()[0].renamed.as_deref(), Some(dst.join("folder (2)").as_path()));
        assert_eq!(fs::read(dst.join("folder/keep.txt")).unwrap(), b"keep");
        assert_eq!(fs::read(dst.join("folder (2)/a.txt")).unwrap(), b"alpha");

        // The reviewed file is replaced by another one with the same name.
        let reviewed = item(src.join("file.txt"));
        fs::remove_file(src.join("file.txt")).unwrap();
        fs::write(src.join("other.txt"), b"placeholder keeps the old inode busy").unwrap();
        fs::write(src.join("file.txt"), b"a different file, longer than before").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let p = run(OpKind::Move, vec![reviewed], Some(dst.clone()));
        wait(&p);
        assert!(p.results.lock()[0].error.is_some());
        assert_eq!(fs::read(src.join("file.txt")).unwrap(), b"a different file, longer than before");
    }

    #[test]
    fn an_item_that_no_longer_matches_the_scan_is_refused() {
        let (_d, src, dst) = fixture();
        let scanned = crate::scan::mtime_of(&fs::symlink_metadata(src.join("file.txt")).unwrap());
        assert!(unchanged_since_scan(&src.join("file.txt"), false, scanned, None));
        assert!(!unchanged_since_scan(&src.join("file.txt"), true, scanned, None));
        assert!(!unchanged_since_scan(&src.join("file.txt"), false, scanned - 100, None));
        // A folder in use keeps changing date and is still the same folder;
        // another folder under its name is not.
        assert!(unchanged_since_scan(&src.join("folder"), true, 0, None));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let md = fs::metadata(src.join("folder")).unwrap();
            assert!(unchanged_since_scan(&src.join("folder"), true, 0, Some((md.dev(), md.ino()))));
            assert!(!unchanged_since_scan(&src.join("folder"), true, 0, Some((md.dev(), md.ino() + 1))));
        }
        // The session gives such an item no identity.
        let mut stale = item(src.join("folder"));
        stale.ident = None;
        for kind in [OpKind::Remove, OpKind::Move, OpKind::Copy] {
            let p = run(kind, vec![stale.clone()], Some(dst.clone()));
            wait(&p);
            assert!(p.results.lock()[0].error.as_deref().unwrap().contains("scan"), "{kind:?}");
        }
        assert!(src.join("folder/a.txt").exists());
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn copy_keeps_private_files_private_and_links_as_links() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, src, dst) = fixture();
        fs::write(dir.path().join("outside.txt"), b"outside").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside.txt"), src.join("folder/link")).unwrap();
        fs::set_permissions(src.join("folder/a.txt"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(src.join("folder/sub"), fs::Permissions::from_mode(0o750)).unwrap();
        let p = run(OpKind::Copy, vec![item(src.join("folder"))], Some(dst.clone()));
        wait(&p);
        assert!(p.results.lock()[0].error.is_none(), "{:?}", p.results.lock()[0].error);
        let mode = |p: &str| fs::symlink_metadata(dst.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode("folder/a.txt"), mode("folder/sub")), (0o600, 0o750));
        assert_eq!(fs::read_link(dst.join("folder/link")).unwrap(), dir.path().join("outside.txt"));
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 1, "the staging folder is gone");
    }

    #[test]
    fn a_stopped_copy_leaves_nothing_behind() {
        let (_d, src, dst) = fixture();
        let stop = AtomicBool::new(true);
        assert!(run_one(OpKind::Copy, &item(src.join("folder")), Some(&dst), &stop).is_err());
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 0);
        assert!(src.join("folder/sub/b.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn trashing_is_a_rename_that_never_replaces() {
        let (dir, src, _dst) = fixture();
        let trash = dir.path().join("Trash");
        let from = Dir::open(&src).unwrap();
        for (name, recorded) in [("file.txt", "/somewhere/file.txt"), ("folder", "rel ative/folder")] {
            xdg_trash::put(&from, &cstr(name.as_ref()).unwrap(), &trash, Path::new(recorded)).unwrap();
            assert!(!src.join(name).exists());
        }
        assert_eq!(fs::read(trash.join("files/folder/sub/b.txt")).unwrap(), b"beta");
        let info = fs::read_to_string(trash.join("info/folder.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=rel%20ative/folder\nDeletionDate=2"), "{info}");
        // A second item of the same name gets a numbered one.
        fs::write(src.join("file.txt"), b"two").unwrap();
        xdg_trash::put(&from, &cstr("file.txt".as_ref()).unwrap(), &trash, Path::new("/somewhere/file.txt")).unwrap();
        assert_eq!(fs::read(trash.join("files/file.txt")).unwrap(), b"one");
        assert_eq!(fs::read(trash.join("files/file (2).txt")).unwrap(), b"two");
        assert!(trash.join("info/file (2).txt.trashinfo").exists());
    }

    #[test]
    fn failed_copy_leaves_nothing_behind() {
        let (_d, src, dst) = fixture();
        #[cfg(unix)]
        {
            let fifo = std::ffi::CString::new(src.join("folder/pipe").to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            let p = run(OpKind::Copy, vec![item(src.join("folder"))], Some(dst.clone()));
            wait(&p);
            assert!(p.results.lock()[0].error.is_some());
            assert_eq!(fs::read_dir(&dst).unwrap().count(), 0, "partial copy was left in the destination");
            assert!(src.join("folder/a.txt").exists());
        }
        let _ = (src, dst);
    }

    #[test]
    fn move_relocates() {
        let (_d, src, dst) = fixture();
        let p = run(OpKind::Move, vec![item(src.join("folder"))], Some(dst.clone()));
        wait(&p);
        assert!(p.results.lock()[0].error.is_none());
        assert!(!src.join("folder").exists());
        assert_eq!(fs::read(dst.join("folder/a.txt")).unwrap(), b"alpha");
    }

    #[test]
    fn refuses_destination_inside_source_and_stale_items() {
        let (_d, src, _dst) = fixture();
        let p = run(OpKind::Copy, vec![item(src.join("folder"))], Some(src.join("folder/sub")));
        wait(&p);
        assert!(p.results.lock()[0].error.is_some());

        let mut gone = item(src.join("file.txt"));
        gone.path = src.join("missing.txt");
        let p = run(OpKind::Remove, vec![gone], None);
        wait(&p);
        assert!(p.results.lock()[0].error.is_some());

        // A folder replaced by a file since the scan is refused.
        let mut swapped = item(src.join("file.txt"));
        swapped.is_dir = true;
        let p = run(OpKind::Remove, vec![swapped], None);
        wait(&p);
        assert!(p.results.lock()[0].error.is_some());
        assert!(src.join("file.txt").exists());
    }

    #[test]
    fn system_locations_are_protected() {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        assert!(protected_reason(Path::new(root)).is_some());
        if let Some(home) = dirs::home_dir() {
            assert!(protected_reason(&home).is_some());
            assert!(protected_reason(&home.join("some-project/build")).is_none());
        }
        #[cfg(target_os = "macos")]
        {
            assert!(protected_reason(Path::new("/System/Library/Fonts")).is_some());
            assert!(protected_reason(Path::new("/Applications")).is_some());
            assert!(protected_reason(Path::new("/Applications/Foo.app")).is_none());
            assert!(protected_reason(Path::new("/private/tmp/x/y")).is_none());
            assert!(protected_reason(Path::new("/SYSTEM/Library")).is_some(), "case is ignored on macOS");
            assert!(protected_reason(Path::new("/private/var/folders/zz")).is_some(), "other users' temp folders");
            assert!(protected_reason(Path::new("/tmp/../etc/hosts")).is_some());
            // A link in a temp folder must not open a way into a system folder.
            let dir = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink("/etc", dir.path().join("link")).unwrap();
            assert!(protected_reason(&dir.path().join("link/hosts")).is_some());
            assert!(protected_reason(&dir.path().join("link")).is_none(), "the link itself is the user's");
        }
        #[cfg(target_os = "linux")]
        assert!(protected_reason(Path::new("/usr/lib/x")).is_some());
    }

    /// Removal on volumes other than the home one; run explicitly on Linux
    /// with `NO_TRASH_DIR` set to a writable folder of a volume whose root is
    /// not writable, and `VOLUME_TRASH_DIR` to a folder of one whose root is.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[ignore]
    fn remove_never_copies_across_volumes() {
        let remove = |dir: &str| {
            let target = PathBuf::from(dir).join("victim.txt");
            fs::write(&target, b"keep me").unwrap();
            let p = run(OpKind::Remove, vec![item(target.clone())], None);
            wait(&p);
            let error = p.results.lock()[0].error.clone();
            (target, error)
        };
        if let Ok(dir) = std::env::var("NO_TRASH_DIR") {
            let (target, error) = remove(&dir);
            assert!(error.as_deref().unwrap().contains("no usable Trash"), "{error:?}");
            assert_eq!(fs::read(target).unwrap(), b"keep me");
        }
        if let Ok(dir) = std::env::var("VOLUME_TRASH_DIR") {
            let (target, error) = remove(&dir);
            assert!(error.is_none(), "{error:?}");
            assert!(!target.exists());
            let uid = unsafe { libc::geteuid() };
            let trashed = PathBuf::from(&dir).join(format!(".Trash-{uid}/files/victim.txt"));
            assert_eq!(fs::read(trashed).unwrap(), b"keep me");
        }
    }

    /// Moves one tiny temp file to the real system Trash; run explicitly.
    #[test]
    #[ignore]
    fn remove_goes_to_trash() {
        let (_d, src, _dst) = fixture();
        let target = src.join("file.txt");
        let p = run(OpKind::Remove, vec![item(target.clone())], None);
        wait(&p);
        assert!(p.results.lock()[0].error.is_none(), "{:?}", p.results.lock()[0].error);
        assert!(!target.exists());
    }
}
