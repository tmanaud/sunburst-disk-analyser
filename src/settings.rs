//! User preferences, stored as JSON in the platform config folder.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

impl Settings {
    pub fn load() -> Settings {
        file()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if self.ephemeral {
            return;
        }
        let Some(f) = file() else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(f, json);
        }
    }
}
