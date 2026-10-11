//! Remove (to the system Trash), move and copy of selected items.
//!
//! Nothing here ever overwrites an existing file or deletes permanently,
//! except for the source of a cross-volume move after a complete copy.

use crate::tree::NodeId;
use parking_lot::Mutex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

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
    /// Identity recorded when the item was put up for review.
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
                let outcome = run_one(kind, &item, dest.as_deref());
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

fn run_one(kind: OpKind, item: &OpItem, dest: Option<&Path>) -> Result<Option<PathBuf>, String> {
    // Time has passed since the review: the item must still be the same object.
    let now = Ident::of(&item.path).ok_or("The item no longer exists")?;
    if now.is_dir != item.is_dir || item.ident.map(|reviewed| reviewed != now).unwrap_or(false) {
        return Err("The item changed since it was reviewed".into());
    }
    match kind {
        OpKind::Remove => {
            if protected_reason(&item.path).is_some() {
                return Err("This item is required by the system".into());
            }
            move_to_trash(&item.path)?;
            Ok(None)
        }
        OpKind::Copy => {
            let dest = dest.ok_or("No destination folder")?;
            check_destination(&item.path, dest)?;
            let staging = staging_path(&item.path, dest)?;
            // `owned` tells whether the staging object was created by this copy;
            // nothing else is ever cleaned up.
            let mut owned = false;
            if let Err(e) = copy_any(&item.path, &staging, Some(&mut owned)) {
                if owned {
                    let _ = remove_any(&staging);
                }
                return Err(e.to_string());
            }
            match publish(&staging, &item.path, dest) {
                Ok((target, renamed)) => Ok(renamed.then_some(target)),
                Err(e) => {
                    let _ = remove_any(&staging);
                    Err(e)
                }
            }
        }
        OpKind::Move => {
            if protected_reason(&item.path).is_some() {
                return Err("This item is required by the system".into());
            }
            let dest = dest.ok_or("No destination folder")?;
            if item.path.parent() == Some(dest) {
                return Err("The item is already in this folder".into());
            }
            check_destination(&item.path, dest)?;
            // A move is a rename and nothing else: across volumes it would mean
            // copy-then-delete, which cannot be made safe against files changing
            // meanwhile, so that case is refused.
            let (target, renamed) = publish(&item.path, &item.path, dest)?;
            Ok(renamed.then_some(target))
        }
    }
}

/// What identifies an item between the review and the operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ident {
    pub is_dir: bool,
    /// (device, inode) on Unix; (length, modification time) elsewhere.
    key: (u64, u64),
}

impl Ident {
    pub fn of(path: &Path) -> Option<Ident> {
        let md = fs::symlink_metadata(path).ok()?;
        #[cfg(unix)]
        let key = {
            use std::os::unix::fs::MetadataExt;
            (md.dev(), md.ino())
        };
        #[cfg(not(unix))]
        let key = {
            let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64);
            (if md.is_dir() { 0 } else { md.len() }, mtime.unwrap_or(0))
        };
        Some(Ident { is_dir: md.is_dir(), key })
    }
}

fn move_to_trash(path: &Path) -> Result<(), String> {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    // The default method scripts Finder, which needs an Automation permission
    // prompt; NSFileManager trashes directly.
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete(path).map_err(|e| e.to_string())
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

/// A name in `dest` that no other program will plausibly use.
fn staging_path(src: &Path, dest: &Path) -> Result<PathBuf, String> {
    let name = src.file_name().ok_or("Invalid item name")?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let mut staged = std::ffi::OsString::from(format!(".sunburst-copy-{}-{nanos}-", std::process::id()));
    staged.push(name);
    Ok(dest.join(staged))
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

/// Renames `from` to a free name in `dest` without ever replacing anything.
/// Returns the final path and whether a numbered name had to be used.
fn publish(from: &Path, named_like: &Path, dest: &Path) -> Result<(PathBuf, bool), String> {
    for n in 1..10_000 {
        let target = candidate(named_like, dest, n)?;
        match rename_noreplace(from, &target) {
            Ok(()) => return Ok((target, n > 1)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) if is_cross_device(&e) => {
                return Err("The destination is on another volume: copy the item, check the copy, then remove the original".into())
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not find a free name in the destination".into())
}

fn is_cross_device(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17) // ERROR_NOT_SAME_DEVICE
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = e;
        false
    }
}

/// Atomic rename that fails with `AlreadyExists` instead of replacing `to`.
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
        let (cf, ct) = (c(from)?, c(to)?);
        #[cfg(target_os = "macos")]
        let rc = unsafe { libc::renamex_np(cf.as_ptr(), ct.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let rc = unsafe { libc::renameat2(libc::AT_FDCWD, cf.as_ptr(), libc::AT_FDCWD, ct.as_ptr(), libc::RENAME_NOREPLACE) };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            // The file system has no exclusive rename (FAT, some network
            // shares): check, then rename. A narrow race remains there.
            Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOSYS) => rename_checked(from, to),
            _ => Err(err),
        }
    }
    #[cfg(windows)]
    {
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
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        rename_checked(from, to)
    }
}

#[cfg(not(windows))]
fn rename_checked(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::rename(from, to),
        Err(e) => Err(e),
        Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
    }
}

/// Copies `src` to `dst`, which must not exist. `owned` is set as soon as the
/// top-level object has been created by this call.
fn copy_any(src: &Path, dst: &Path, mut owned: Option<&mut bool>) -> io::Result<()> {
    let mut created = || {
        if let Some(flag) = owned.as_deref_mut() {
            *flag = true;
        }
    };
    let md = fs::symlink_metadata(src)?;
    let ft = md.file_type();
    if ft.is_symlink() {
        copy_symlink(src, dst)?;
        created();
        Ok(())
    } else if ft.is_dir() {
        fs::create_dir(dst)?;
        created();
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_any(&entry.path(), &dst.join(entry.file_name()), None)?;
        }
        let _ = fs::set_permissions(dst, md.permissions());
        Ok(())
    } else if ft.is_file() {
        // `create_new` guarantees an existing file is never overwritten.
        let mut output = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
        created();
        let mut input = fs::File::open(src)?;
        io::copy(&mut input, &mut output)?;
        let _ = fs::set_permissions(dst, md.permissions());
        if let Ok(t) = md.modified() {
            let _ = output.set_modified(t);
        }
        Ok(())
    } else {
        // Sockets, devices and pipes cannot be copied.
        Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} is a special file", src.display())))
    }
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(src)?, dst)
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

/// Removes a staging object this module created itself.
fn remove_any(p: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
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
        let changed = Ident::of(&src.join("file.txt"));
        if changed != p.results.lock()[0].item.ident {
            assert!(p.results.lock()[0].error.is_some());
            assert!(src.join("file.txt").exists());
        }
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
