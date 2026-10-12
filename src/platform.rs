//! Everything that differs between macOS, Windows and Linux: volumes,
//! well-known folders and handing files over to the system file manager.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocKind {
    Home,
    Volume,
    Desktop,
    Pictures,
    Documents,
    Downloads,
    Folder,
}

#[derive(Clone, Debug)]
pub struct Location {
    pub name: String,
    pub path: PathBuf,
    pub kind: LocKind,
    pub favorite: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub total: u64,
    pub free: u64,
}

pub fn file_manager_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(windows) {
        "Explorer"
    } else {
        "File Manager"
    }
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub fn home() -> Option<Location> {
    let path = dirs::home_dir()?;
    Some(Location { name: display_name(&path), path, kind: LocKind::Home, favorite: false })
}

pub fn standard_folders() -> Vec<Location> {
    let mut v = Vec::new();
    let mut add = |p: Option<PathBuf>, kind| {
        if let Some(path) = p.filter(|p| p.is_dir()) {
            v.push(Location { name: display_name(&path), path, kind, favorite: false });
        }
    };
    add(dirs::desktop_dir(), LocKind::Desktop);
    add(dirs::picture_dir(), LocKind::Pictures);
    add(dirs::document_dir(), LocKind::Documents);
    add(dirs::download_dir(), LocKind::Downloads);
    v
}

/// Mounted volumes worth offering for a scan, with their capacity.
pub fn volumes() -> Vec<(Location, Space)> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<(Location, Space)> = Vec::new();
    for d in disks.list() {
        let mount = d.mount_point().to_path_buf();
        if d.total_space() == 0 || !is_user_volume(&mount) {
            continue;
        }
        if out.iter().any(|(l, _)| l.path == mount) {
            continue;
        }
        let mut name = d.name().to_string_lossy().into_owned();
        if cfg!(windows) {
            let letter = mount.to_string_lossy().trim_end_matches('\\').to_string();
            name = if name.is_empty() { letter } else { format!("{name} ({letter})") };
        } else if cfg!(target_os = "linux") || name.is_empty() {
            name = if mount == Path::new("/") { root_volume_name() } else { display_name(&mount) };
        }
        out.push((
            Location { name, path: mount, kind: LocKind::Volume, favorite: false },
            Space { total: d.total_space(), free: d.available_space() },
        ));
    }
    out.sort_by(|a, b| {
        let root = |l: &Location| l.path != Path::new("/") && l.path != Path::new("C:\\");
        root(&a.0).cmp(&root(&b.0)).then_with(|| a.0.name.cmp(&b.0.name))
    });
    out
}

fn root_volume_name() -> String {
    if cfg!(target_os = "macos") {
        "Macintosh HD".into()
    } else {
        "File System".into()
    }
}

fn is_user_volume(mount: &Path) -> bool {
    if cfg!(target_os = "macos") {
        mount == Path::new("/") || (mount.starts_with("/Volumes") && !display_name(mount).starts_with("com.apple."))
    } else if cfg!(target_os = "linux") {
        const SKIP: &[&str] = &["/boot", "/snap", "/sys", "/proc", "/dev", "/run/lock", "/run/snapd", "/var/lib/docker", "/var/snap"];
        mount == Path::new("/") || !SKIP.iter().any(|s| mount.starts_with(s))
    } else {
        true
    }
}

/// Capacity of the volume mounted exactly at `path`, if it is one.
pub fn volume_space(path: &Path) -> Option<Space> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .find(|d| d.mount_point() == path && d.total_space() > 0)
        .map(|d| Space { total: d.total_space(), free: d.available_space() })
}

/// Selects the item in Finder / Explorer / the default file manager.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        let _ = Command::new("explorer").arg(arg).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::os::unix::ffi::OsStrExt;
        // Percent-encode everything but unreserved characters and `/`: commas
        // would otherwise split dbus-send's array argument.
        let mut uri = String::from("file://");
        for &b in path.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
                uri.push(b as char);
            } else {
                uri.push_str(&format!("%{b:02X}"));
            }
        }
        let parent = path.parent().unwrap_or(path).to_path_buf();
        // The file manager may answer slowly: never wait on the UI thread.
        std::thread::spawn(move || {
            let shown = Command::new("dbus-send")
                .args([
                    "--session",
                    "--print-reply",
                    "--reply-timeout=3000",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                    &format!("array:string:{uri}"),
                    "string:",
                ])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !shown {
                let _ = open::that_detached(parent);
            }
        });
    }
}

/// Previews the item: Quick Look on macOS, the default application elsewhere.
pub fn quick_look(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("qlmanage")
            .arg("-p")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Opening a launcher runs the command written in it: show it instead.
        if cfg!(unix) && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("desktop")) {
            reveal(path);
            return;
        }
        let _ = open::that_detached(path);
    }
}

/// Opens the item with the application the system associates with it.
pub fn open_file(path: &Path) {
    let _ = open::that_detached(path);
}

pub fn preview_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Quick Look"
    } else {
        "Open"
    }
}

/// Opens the system page where disk access can be granted (macOS only).
pub fn open_disk_access_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn();
    }
}

pub fn created_secs(path: &Path) -> i64 {
    secs(std::fs::metadata(path).and_then(|m| m.created()))
}

pub fn modified_secs(path: &Path) -> i64 {
    secs(std::fs::metadata(path).and_then(|m| m.modified()))
}

fn secs(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Moves the close, minimise and zoom buttons of the window so that they start
/// at `left` and are centred on `centre_y`, both in points from the top-left
/// corner. Returns their right edge, or `None` when the window shows none
/// (full screen). The system resets them on every resize: call this each frame.
#[cfg(target_os = "macos")]
pub fn place_window_buttons(frame: &eframe::Frame, left: f32, centre_y: f32) -> Option<f32> {
    use objc2::encode::{Encode, Encoding};
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    unsafe impl Encode for Point {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for Size {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for Rect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
    }
    const FULL_SCREEN: usize = 1 << 14;

    let RawWindowHandle::AppKit(handle) = frame.window_handle().ok()?.as_raw() else { return None };
    let view = handle.ns_view.as_ptr() as *mut AnyObject;
    // SAFETY: the view belongs to the live window of this frame, `update` runs on the
    // main thread, and every object is checked for nil before it receives a message.
    unsafe {
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() {
            return None;
        }
        let style: usize = msg_send![window, styleMask];
        if style & FULL_SCREEN != 0 {
            return None;
        }
        let mut buttons = [std::ptr::null_mut::<AnyObject>(); 3];
        for (kind, button) in buttons.iter_mut().enumerate() {
            *button = msg_send![window, standardWindowButton: kind];
            if button.is_null() {
                return None;
            }
        }
        let bar: *mut AnyObject = msg_send![buttons[0], superview];
        if bar.is_null() {
            return None;
        }
        let container: *mut AnyObject = msg_send![bar, superview];
        if container.is_null() {
            return None;
        }
        // Where the buttons are now, in window coordinates (origin at the bottom left).
        let nowhere = std::ptr::null::<AnyObject>();
        let outer: Rect = msg_send![window, frame];
        let bounds: Rect = msg_send![buttons[0], bounds];
        let seen: Rect = msg_send![buttons[0], convertRect: bounds, toView: nowhere];
        // The whole title bar moves down with them, so that they stay inside it.
        let down = centre_y as f64 - (outer.size.h - seen.origin.y - seen.size.h * 0.5);
        if down.abs() > 0.5 {
            let mut area: Rect = msg_send![container, frame];
            area.origin.y -= down;
            let _: () = msg_send![container, setFrame: area];
        }
        let right = left as f64 - seen.origin.x;
        let first: Rect = msg_send![buttons[0], frame];
        let second: Rect = msg_send![buttons[1], frame];
        let step = second.origin.x - first.origin.x;
        if right.abs() > 0.5 {
            for button in buttons {
                let at: Rect = msg_send![button, frame];
                let _: () = msg_send![button, setFrameOrigin: Point { x: at.origin.x + right, y: at.origin.y }];
            }
        }
        Some((left as f64 + step * 2.0 + first.size.w) as f32)
    }
}
