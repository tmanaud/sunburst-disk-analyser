//! Preset filters for the search: file type, minimum size, modification date.

use crate::tree::Node;

/// Groups of file extensions, one bit each in `Filter::kinds`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Image,
    Audio,
    Document,
    Archive,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Video, Kind::Image, Kind::Audio, Kind::Document, Kind::Archive];

    pub fn bit(self) -> u8 {
        1 << self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Video => "Videos",
            Kind::Image => "Images",
            Kind::Audio => "Audio",
            Kind::Document => "Documents",
            Kind::Archive => "Archives & disk images",
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Kind::Video => &["mp4", "m4v", "mov", "mkv", "avi", "wmv", "flv", "webm", "mpg", "mpeg", "m2ts", "mts", "ts", "3gp", "vob", "braw", "r3d", "mxf"],
            Kind::Image => &[
                "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "heic", "heif", "webp", "avif", "svg", "psd", "psb", "ai", "raw", "dng", "cr2", "cr3", "nef",
                "arw", "orf", "rw2", "raf", "exr", "hdr",
            ],
            Kind::Audio => &["mp3", "wav", "flac", "aac", "m4a", "aif", "aiff", "ogg", "opus", "wma", "alac", "caf", "mid", "midi"],
            Kind::Document => &[
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "pages", "numbers", "key", "rtf", "txt", "md", "csv", "epub",
            ],
            Kind::Archive => &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "dmg", "iso", "img", "pkg", "vhd", "vhdx", "vmdk", "qcow2"],
        }
    }
}

/// Bounds on the modification date, in days before now.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Age {
    #[default]
    Any,
    OlderThan(u32),
    NewerThan(u32),
}

impl Age {
    pub const PRESETS: [Age; 5] = [Age::OlderThan(182), Age::OlderThan(365), Age::OlderThan(3 * 365), Age::NewerThan(7), Age::NewerThan(30)];

    pub fn label(self) -> &'static str {
        match self {
            Age::Any => "Any date",
            Age::OlderThan(182) => "Older than 6 months",
            Age::OlderThan(365) => "Older than 1 year",
            Age::OlderThan(_) => "Older than 3 years",
            Age::NewerThan(7) => "Last 7 days",
            Age::NewerThan(_) => "Last 30 days",
        }
    }
}

/// Minimum sizes offered, in bytes (decimal units, as shown everywhere else).
pub const MIN_SIZES: [(u64, &str); 4] = [(10_000_000, "10 MB"), (100_000_000, "100 MB"), (1_000_000_000, "1 GB"), (10_000_000_000, "10 GB")];

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Filter {
    /// `Kind` bits; 0 = any type.
    pub kinds: u8,
    /// 0 = any size.
    pub min_size: u64,
    pub age: Age,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    /// Type and date only make sense for files: a folder's own date says
    /// nothing about what it holds. The minimum size applies to folders too.
    pub fn matches(&self, n: &Node, now: i64) -> bool {
        if n.size < self.min_size {
            return false;
        }
        if self.kinds == 0 && self.age == Age::Any {
            return true;
        }
        if n.is_dir() {
            return false;
        }
        match self.age {
            Age::Any => {}
            // An unknown date matches neither bound.
            _ if n.mtime <= 0 => return false,
            Age::OlderThan(days) if n.mtime > now - days as i64 * 86_400 => return false,
            Age::NewerThan(days) if n.mtime < now - days as i64 * 86_400 => return false,
            _ => {}
        }
        self.kinds == 0 || self.kind_matches(&n.name.to_string_lossy())
    }

    fn kind_matches(&self, name: &str) -> bool {
        let Some((_, ext)) = name.rsplit_once('.') else { return false };
        Kind::ALL
            .iter()
            .filter(|k| self.kinds & k.bit() != 0)
            .any(|k| k.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
    }
}

/// Current time in seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
