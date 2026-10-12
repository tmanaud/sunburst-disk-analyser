//! User preferences, stored as JSON in the platform config folder.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum Appearance {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    /// Language code, or "auto" to follow the system.
    pub language: String,
    pub allow_removing_hidden: bool,
    pub show_selection_hint: bool,
    pub favorites: Vec<PathBuf>,
    pub sidebar_open: bool,
    pub show_chart: bool,
    /// Set for demo and screenshot runs, which must not touch the saved preferences.
    #[serde(skip)]
    pub ephemeral: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            appearance: Appearance::Auto,
            language: "auto".into(),
            allow_removing_hidden: false,
            show_selection_hint: true,
            favorites: Vec::new(),
            sidebar_open: true,
            show_chart: true,
            ephemeral: false,
        }
    }
}

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("sunburst-disk-analyser").join("settings.json"))
}

/// A settings file is a few hundred bytes; anything far larger is not ours.
const MAX_FILE_SIZE: u64 = 1 << 20;
const MAX_FAVORITES: usize = 200;

impl Settings {
    pub fn load() -> Settings {
        file().and_then(|f| Self::load_from(&f)).unwrap_or_default()
    }

    fn load_from(file: &Path) -> Option<Settings> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(file).ok()?.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > MAX_FILE_SIZE {
            return None;
        }
        let mut settings: Settings = serde_json::from_slice(&bytes).ok()?;
        settings.favorites.truncate(MAX_FAVORITES);
        Some(settings)
    }

    pub fn save(&self) {
        if self.ephemeral {
            return;
        }
        if let Some(f) = file() {
            let _ = self.save_to(&f);
        }
    }

    /// Writes a new file and renames it into place, so a crash half-way
    /// never leaves a truncated settings file. The file lists folder paths
    /// and is readable by its owner only.
    fn save_to(&self, file: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let dir = file.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            builder.mode(0o700);
            options.mode(0o600);
        }
        builder.create(dir)?;
        let temp = file.with_extension(format!("json.{}.tmp", std::process::id()));
        let written = options
            .open(&temp)
            .and_then(|mut out| out.write_all(&serde_json::to_vec_pretty(self)?))
            .and_then(|()| std::fs::rename(&temp, file));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_reject_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config/settings.json");
        let mut settings = Settings { language: "fr".into(), ..Settings::default() };
        settings.favorites = (0..300).map(|i| PathBuf::from(format!("/fav/{i}"))).collect();
        settings.save_to(&file).unwrap();
        settings.save_to(&file).unwrap();
        assert_eq!(std::fs::read_dir(file.parent().unwrap()).unwrap().count(), 1, "no temporary file is left");
        let loaded = Settings::load_from(&file).unwrap();
        assert_eq!((loaded.language.as_str(), loaded.favorites.len()), ("fr", MAX_FAVORITES));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&file, vec![b' '; MAX_FILE_SIZE as usize + 1]).unwrap();
        assert!(Settings::load_from(&file).is_none());
    }
}
