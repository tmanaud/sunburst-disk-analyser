//! Directory handles for file access that cannot be redirected.
//!
//! Every name is resolved relative to an open directory and never through a
//! symbolic link, so swapping a folder for a link while a scan or a copy is
//! running cannot send it somewhere else.

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

/// `readdir` reports errors through `errno` only, which must be cleared first.
fn clear_errno() {
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn readdir_failed() -> bool {
    if cfg!(any(target_os = "macos", target_os = "linux")) {
        io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0
    } else {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    File,
    Symlink,
    /// Socket, device or pipe.
    Other,
}

/// What `lstat` reports, reduced to the fields the app uses.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub kind: Kind,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub uid: u32,
    /// Permission bits only.
    pub mode: u32,
    pub len: u64,
    pub blocks: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    /// The macOS "hidden" file flag.
    pub hidden: bool,
}

impl Stat {
    #[allow(clippy::unnecessary_cast)]
    fn from(st: &libc::stat) -> Stat {
        let kind = match st.st_mode as u32 & libc::S_IFMT as u32 {
            m if m == libc::S_IFDIR as u32 => Kind::Dir,
            m if m == libc::S_IFREG as u32 => Kind::File,
            m if m == libc::S_IFLNK as u32 => Kind::Symlink,
            _ => Kind::Other,
        };
        #[cfg(target_os = "macos")]
        let hidden = st.st_flags & 0x8000 != 0; // UF_HIDDEN
        #[cfg(not(target_os = "macos"))]
        let hidden = false;
        Stat {
            kind,
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
            nlink: st.st_nlink as u64,
            uid: st.st_uid as u32,
            mode: st.st_mode as u32 & 0o7777,
            len: st.st_size as u64,
            blocks: st.st_blocks as u64,
            mtime: st.st_mtime as i64,
            mtime_nsec: st.st_mtime_nsec as i64,
            hidden,
        }
    }

    pub fn of_file(f: &File) -> io::Result<Stat> {
        fstat(f.as_raw_fd())
    }
}

fn fstat(fd: RawFd) -> io::Result<Stat> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
    check(unsafe { libc::fstat(fd, st.as_mut_ptr()) })?;
    Ok(Stat::from(unsafe { &st.assume_init() }))
}

/// Applies permission bits to an open file or folder. File systems without
/// permissions (FAT, some network shares) refuse; that is not an error.
pub fn set_mode(fd: RawFd, mode: u32) -> io::Result<()> {
    match check(unsafe { libc::fchmod(fd, mode as libc::mode_t) }) {
        Ok(_) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(libc::EPERM) | Some(libc::ENOTSUP) | Some(libc::EINVAL)) => Ok(()),
        Err(e) => Err(e),
    }
}

pub struct Dir(OwnedFd);

impl AsRawFd for Dir {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl Dir {
    fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    fn open_with(path: &Path, extra: libc::c_int) -> io::Result<Dir> {
        let c = cstr(path.as_os_str())?;
        let fd = check(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | extra) })?;
        Ok(Dir(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Opens a folder by path; links in the path are followed.
    pub fn open(path: &Path) -> io::Result<Dir> {
        Dir::open_with(path, 0)
    }

    /// Opens a folder by path, refusing a link as the last component.
    pub fn open_nofollow(path: &Path) -> io::Result<Dir> {
        Dir::open_with(path, libc::O_NOFOLLOW)
    }

    pub fn try_clone(&self) -> io::Result<Dir> {
        let fd = check(unsafe { libc::fcntl(self.fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
        Ok(Dir(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    pub fn stat_self(&self) -> io::Result<Stat> {
        fstat(self.fd())
    }

    /// `lstat` of an entry of this folder.
    pub fn stat(&self, name: &CStr) -> io::Result<Stat> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
        check(unsafe { libc::fstatat(self.fd(), name.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) })?;
        Ok(Stat::from(unsafe { &st.assume_init() }))
    }

    /// Entry names, at most `max` of them. The flag is false when the listing
    /// is incomplete: limit reached, `stop` returned true or a read error.
    pub fn names(&self, max: usize, stop: &dyn Fn() -> bool) -> io::Result<(Vec<CString>, bool)> {
        // `fdopendir` takes its descriptor over, and a duplicate shares the
        // read position of the original.
        let dup = check(unsafe { libc::fcntl(self.fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
        let dirp = unsafe { libc::fdopendir(dup) };
        if dirp.is_null() {
            let e = io::Error::last_os_error();
            unsafe { libc::close(dup) };
            return Err(e);
        }
        unsafe { libc::rewinddir(dirp) };
        let mut names = Vec::new();
        let complete = loop {
            clear_errno();
            let ent = unsafe { libc::readdir(dirp) };
            if ent.is_null() {
                break !readdir_failed();
            }
            let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            if names.len() >= max || (names.len() % 1024 == 0 && stop()) {
                break false;
            }
            names.push(name.to_owned());
        };
        unsafe { libc::closedir(dirp) };
        Ok((names, complete))
    }

    fn open_at(&self, name: &CStr, flags: libc::c_int, mode: libc::c_uint) -> io::Result<OwnedFd> {
        let fd = check(unsafe { libc::openat(self.fd(), name.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode) })?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Opens a sub-folder; fails when the entry is a link.
    pub fn open_dir(&self, name: &CStr) -> io::Result<Dir> {
        self.open_at(name, libc::O_RDONLY | libc::O_DIRECTORY, 0).map(Dir)
    }

    /// Opens an entry for reading without following a link and without
    /// waiting, should it have been swapped for a pipe.
    pub fn open_file(&self, name: &CStr) -> io::Result<File> {
        match self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            // A cloud-only file that has to be downloaded first.
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && self.stat(name)?.kind == Kind::File => {
                self.open_at(name, libc::O_RDONLY, 0).map(File::from)
            }
            r => r.map(File::from),
        }
    }

    /// Creates a file readable by its owner only; never replaces anything.
    pub fn create_file(&self, name: &CStr) -> io::Result<File> {
        self.open_at(name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600).map(File::from)
    }

    /// Creates a folder accessible by its owner only.
    pub fn create_dir(&self, name: &CStr) -> io::Result<()> {
        check(unsafe { libc::mkdirat(self.fd(), name.as_ptr(), 0o700) }).map(drop)
    }

    pub fn read_link(&self, name: &CStr) -> io::Result<CString> {
        let mut buf = vec![0u8; 1024];
        loop {
            let n = unsafe { libc::readlinkat(self.fd(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            if (n as usize) < buf.len() {
                buf.truncate(n as usize);
                return CString::new(buf).map_err(|_| io::Error::from(io::ErrorKind::InvalidData));
            }
            buf.resize(buf.len() * 2, 0);
        }
    }

    pub fn symlink(&self, target: &CStr, name: &CStr) -> io::Result<()> {
        check(unsafe { libc::symlinkat(target.as_ptr(), self.fd(), name.as_ptr()) }).map(drop)
    }

    /// Removes an entry; a folder must be empty.
    pub fn unlink(&self, name: &CStr, dir: bool) -> io::Result<()> {
        check(unsafe { libc::unlinkat(self.fd(), name.as_ptr(), if dir { libc::AT_REMOVEDIR } else { 0 }) }).map(drop)
    }

    /// Renames an entry into `to` without ever replacing something that was
    /// already there: fails with `AlreadyExists` instead.
    pub fn rename_excl(&self, name: &CStr, to: &Dir, to_name: &CStr) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        let rc = unsafe { libc::renameatx_np(self.fd(), name.as_ptr(), to.fd(), to_name.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let rc = unsafe { libc::renameat2(self.fd(), name.as_ptr(), to.fd(), to_name.as_ptr(), libc::RENAME_NOREPLACE) };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return self.rename_reserved(name, to, to_name);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            if rc == 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                // The file system has no exclusive rename (FAT, some network shares).
                Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOSYS) => self.rename_reserved(name, to, to_name),
                _ => Err(err),
            }
        }
    }

    /// Exclusive rename for file systems that lack one: the target name is
    /// first taken with an exclusive create, then the item is renamed over
    /// that placeholder, so only an object made here can be replaced.
    fn rename_reserved(&self, name: &CStr, to: &Dir, to_name: &CStr) -> io::Result<()> {
        let rename = || check(unsafe { libc::renameat(self.fd(), name.as_ptr(), to.fd(), to_name.as_ptr()) });
        if self.stat(name)?.kind == Kind::Dir {
            to.create_dir(to_name)?;
            // A folder can only be renamed over an empty folder.
            if let Err(e) = rename() {
                let _ = to.unlink(to_name, true);
                return Err(e);
            }
        } else {
            let mine = Stat::of_file(&to.create_file(to_name)?)?;
            if let Err(e) = rename() {
                if to.stat(to_name).map(|now| (now.dev, now.ino, now.len) == (mine.dev, mine.ino, 0)).unwrap_or(false) {
                    let _ = to.unlink(to_name, false);
                }
                return Err(e);
            }
        }
        Ok(())
    }

    /// Deletes everything inside this folder, through handles only.
    pub fn clear(&self) -> io::Result<()> {
        struct Frame {
            dir: Dir,
            names: Vec<CString>,
            /// Name in the parent frame.
            own: Option<CString>,
        }
        let all = |d: &Dir| d.names(usize::MAX, &|| false).map(|(names, _)| names);
        let mut stack = vec![Frame { dir: self.try_clone()?, names: all(self)?, own: None }];
        while let Some(top) = stack.last_mut() {
            if let Some(name) = top.names.pop() {
                if top.dir.stat(&name)?.kind == Kind::Dir {
                    let child = top.dir.open_dir(&name)?;
                    let _ = set_mode(child.fd(), 0o700);
                    let names = all(&child)?;
                    stack.push(Frame { dir: child, names, own: Some(name) });
                } else {
                    top.dir.unlink(&name, false)?;
                }
            } else {
                let done = stack.pop().expect("frame");
                match (stack.last(), done.own) {
                    (Some(parent), Some(name)) => parent.dir.unlink(&name, true)?,
                    _ => break,
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn links_are_never_followed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("real")).unwrap();
        std::fs::write(tmp.path().join("real/secret"), b"x").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("dirlink")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real/secret"), tmp.path().join("filelink")).unwrap();
        let dir = Dir::open(tmp.path()).unwrap();
        assert!(dir.open_dir(&c("dirlink")).is_err());
        assert!(dir.open_file(&c("filelink")).is_err());
        assert!(Dir::open_nofollow(&tmp.path().join("dirlink")).is_err());
        assert_eq!(dir.stat(&c("dirlink")).unwrap().kind, Kind::Symlink);
        let (mut names, complete) = dir.names(usize::MAX, &|| false).unwrap();
        names.sort();
        assert!(complete);
        assert_eq!(names, vec![c("dirlink"), c("filelink"), c("real")]);
        assert!(!dir.names(2, &|| false).unwrap().1, "the limit is reported");
    }

    #[test]
    fn rename_never_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Dir::open(tmp.path()).unwrap();
        for exclusive in [true, false] {
            let rename = |a: &str, b: &str| if exclusive { dir.rename_excl(&c(a), &dir, &c(b)) } else { dir.rename_reserved(&c(a), &dir, &c(b)) };
            std::fs::write(tmp.path().join("a"), b"new").unwrap();
            std::fs::write(tmp.path().join("b"), b"kept").unwrap();
            assert_eq!(rename("a", "b").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read(tmp.path().join("b")).unwrap(), b"kept");
            rename("a", "c").unwrap();
            assert_eq!(std::fs::read(tmp.path().join("c")).unwrap(), b"new");

            std::fs::create_dir(tmp.path().join("d1")).unwrap();
            std::fs::write(tmp.path().join("d1/x"), b"x").unwrap();
            std::fs::create_dir(tmp.path().join("d2")).unwrap();
            assert_eq!(rename("d1", "d2").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
            rename("d1", "d3").unwrap();
            assert!(tmp.path().join("d3/x").exists());
            dir.clear().unwrap();
            assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn created_objects_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = Dir::open(tmp.path()).unwrap();
        dir.create_dir(&c("d")).unwrap();
        drop(dir.create_file(&c("f")).unwrap());
        assert!(dir.create_file(&c("f")).is_err());
        let mode = |n: &str| std::fs::metadata(tmp.path().join(n)).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode("d"), mode("f")), (0o700, 0o600));
    }
}
