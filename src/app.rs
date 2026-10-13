//! Application state, commands and the per-frame update loop.

use crate::i18n::{self, t, tf};
use crate::ops::{self, OpKind, OpProgress};
use crate::platform::{self, LocKind, Location, Space};
use crate::scan::{self, ScanHandle};
use crate::session::{CheckError, GroupKind, Session, Step, Tab};
use crate::swipe::Swipe;
use crate::settings::{Appearance, Settings};
use crate::theme::{self, Palette};
use crate::tree::NodeId;
use crate::{demo, export, util};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Command line options.
#[derive(Default, Clone)]
pub struct Options {
    /// Show synthetic data instead of the real disk.
    pub demo: bool,
    /// Render one scene to a PNG file and quit: `(scene, output)`.
    pub shot: Option<(String, PathBuf)>,
    /// Folder to scan right away.
    pub scan: Option<PathBuf>,
    pub lang: Option<String>,
    pub theme: Option<String>,
}

/// Commands reachable from menus, shortcuts and buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Cmd {
    Scan,
    ScanCustom,
    Stop,
    ExportCsv,
    AddFavorite,
    Reveal,
    Info,
    QuickLook,
    CopyPath,
    Find,
    ToggleSidebar,
    TabOutline,
    TabBiggest,
    ToggleChart,
    GoBack,
    GoForward,
    GoParent,
    GoRoot,
    Remove,
    Move,
    Copy,
    DeselectAll,
    Settings,
    About,
    Help,
    Quit,
}

/// Things the views ask for while painting; applied after the frame is drawn.
#[derive(Clone, Debug)]
pub enum Action {
    Cmd(Cmd),
    SelectLocation(Location),
    RemoveFavorite(PathBuf),
    RowClick { col: usize, id: NodeId },
    FlatClick(NodeId),
    SelectPath(NodeId),
    Step(Step),
    /// Open a file with its default application.
    Open(NodeId),
    Hover(NodeId, bool),
    ToggleCheck(NodeId),
    ToggleGroup(NodeId, GroupKind),
    Crumb(NodeId),
    MoreBiggest,
    SetQuery(String),
    CloseSearch,
    /// A command applied to a specific item (context menus).
    On(Cmd, NodeId),
    QuickRemove(NodeId),
    SetOpMode(OpKind),
    Review,
    ShowDenied,
    DismissHint,
    /// A pane edge was dragged: new width of the outline columns, or of the flat lists.
    /// `done` ends the drag.
    PaneWidth { flat: bool, width: f32, done: bool },
}

pub struct ReviewItem {
    pub item: ops::OpItem,
    pub name: String,
    pub parent: String,
    pub hidden: bool,
}

pub struct InfoData {
    pub name: String,
    pub path: PathBuf,
    pub kind: &'static str,
    pub size: Option<u64>,
    pub items: Option<u32>,
    pub created: i64,
    pub modified: i64,
    pub flags: u8,
}

pub struct Summary {
    pub kind: OpKind,
    pub ok: usize,
    pub size: u64,
    pub failed: Vec<(String, String)>,
    pub renamed: Vec<(String, String)>,
}

pub enum Dialog {
    None,
    Review { kind: OpKind, items: Vec<ReviewItem>, dest: Option<PathBuf>, session: (PathBuf, u64) },
    Progress { progress: Arc<OpProgress>, session: (PathBuf, u64) },
    Summary(Summary),
    Info(InfoData),
    Settings,
    About,
    Help,
    Message { title: String, body: String },
}

pub struct App {
    pub pal: Palette,
    pub settings: Settings,
    pub opts: Options,
    pub home: Option<Location>,
    pub volumes: Vec<(Location, Space)>,
    pub folders: Vec<Location>,
    /// Custom folders scanned in this run that are not favourites.
    pub recent: Vec<Location>,
    pub current: Option<Location>,
    pub sessions: HashMap<PathBuf, Session>,
    pub dialog: Dialog,
    pub op_mode: OpKind,
    pub toast: Option<(String, Instant)>,
    pub hint_dismissed: bool,
    pub actions: Vec<Action>,
    pub hover_lock: bool,
    pub focus_search: bool,
    /// Right edge of the macOS window buttons, 0 when the window shows none.
    pub lights: f32,
    pub thumbs: crate::thumb::Thumbs,
    swipe: Swipe,
    /// Time of the last trackpad scroll event.
    trackpad_at: f64,
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::menu::NativeMenu>,
    applied_dark: Option<bool>,
    applied_lang: Option<i18n::Lang>,
    volumes_checked: Instant,
    /// Whether protected folders can be read without a system prompt, see `platform::has_full_disk_access`.
    pub disk_access: bool,
    frame: u64,
    shot_requested: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: Options) -> App {
        theme::install_fonts(&cc.egui_ctx);
        let mut settings = if opts.demo { Settings { ephemeral: true, ..Settings::default() } } else { Settings::load() };
        if let Some(l) = &opts.lang {
            settings.language = l.clone();
        }
        match opts.theme.as_deref() {
            Some("light") => settings.appearance = Appearance::Light,
            Some("dark") => settings.appearance = Appearance::Dark,
            _ => {}
        }
        let mut app = App {
            pal: Palette::dark(),
            settings,
            opts: opts.clone(),
            home: None,
            volumes: Vec::new(),
            folders: Vec::new(),
            recent: Vec::new(),
            current: None,
            sessions: HashMap::new(),
            dialog: Dialog::None,
            op_mode: OpKind::Remove,
            toast: None,
            hint_dismissed: false,
            actions: Vec::new(),
            hover_lock: false,
            focus_search: false,
            lights: if cfg!(target_os = "macos") { 76.0 } else { 0.0 },
            thumbs: Default::default(),
            swipe: Swipe::default(),
            trackpad_at: f64::NEG_INFINITY,
            #[cfg(target_os = "macos")]
            native_menu: None,
            applied_dark: None,
            applied_lang: None,
            volumes_checked: Instant::now(),
            disk_access: opts.demo || platform::has_full_disk_access(),
            frame: 0,
            shot_requested: false,
        };
        if opts.demo {
            app.load_demo();
        } else {
            app.home = platform::home();
            app.volumes = platform::volumes();
            app.folders = platform::standard_folders();
            app.current = app.home.clone().or_else(|| app.volumes.first().map(|v| v.0.clone()));
            if let Some(path) = opts.scan.clone().filter(|p| p.is_dir()) {
                let path = std::fs::canonicalize(&path).map(|p| ops::plain(&p)).unwrap_or(path);
                app.scan_folder(path);
            } else if app.settings.scan_home_at_launch && app.disk_access && app.home.is_some() {
                // Without full access, an unrequested scan would open one system prompt per protected folder.
                app.current = app.home.clone();
                app.start_scan();
            }
        }
        app
    }

    fn load_demo(&mut self) {
        let home = demo::home_path();
        let loc = |name: &str, path: PathBuf, kind| Location { name: name.into(), path, kind, favorite: false };
        self.home = Some(loc(demo::HOME_NAME, home.clone(), LocKind::Home));
        let disk = if cfg!(windows) { ("Local Disk (C:)", "C:\\") } else { ("Macintosh HD", "/") };
        self.volumes = vec![(
            loc(disk.0, PathBuf::from(disk.1), LocKind::Volume),
            Space { total: 2_000_000_000_000, free: 352_000_000_000 },
        )];
        self.folders = vec![
            loc("Desktop", home.join("Desktop"), LocKind::Desktop),
            loc("Pictures", home.join("Pictures"), LocKind::Pictures),
            loc("Documents", home.join("Documents"), LocKind::Documents),
        ];
        self.current = self.home.clone();
        let session = Session::new(demo::HOME_NAME.into(), ScanHandle::finished(demo::tree()), None);
        self.sessions.insert(home, session);
    }

    // ---- locations ----------------------------------------------------------

    pub fn favorites(&self) -> Vec<Location> {
        self.settings
            .favorites
            .iter()
            .map(|p| Location { name: platform::display_name(p), path: p.clone(), kind: LocKind::Folder, favorite: true })
            .collect()
    }

    pub fn space_of(&self, path: &Path) -> Option<Space> {
        self.volumes.iter().find(|v| v.0.path == path).map(|v| v.1)
    }

    fn known_location(&self, path: &Path) -> Option<Location> {
        self.home
            .iter()
            .chain(self.volumes.iter().map(|v| &v.0))
            .chain(self.folders.iter())
            .chain(self.recent.iter())
            .find(|l| l.path == path)
            .cloned()
            .or_else(|| self.favorites().into_iter().find(|l| l.path == path))
    }

    pub fn session(&self) -> Option<&Session> {
        self.current.as_ref().and_then(|l| self.sessions.get(&l.path))
    }

    fn session_mut(&mut self) -> Option<&mut Session> {
        let path = self.current.as_ref()?.path.clone();
        self.sessions.get_mut(&path)
    }

    pub fn is_scanning(&self) -> bool {
        self.session().map(|s| !s.scan.is_done()).unwrap_or(false)
    }

    // ---- scanning -----------------------------------------------------------

    fn start_scan(&mut self) {
        let Some(loc) = self.current.clone() else { return };
        if self.opts.demo {
            self.load_demo();
            return;
        }
        if let Some(old) = self.sessions.get(&loc.path) {
            old.scan.cancel();
        }
        let space = platform::volume_space(&loc.path).filter(|_| loc.kind == LocKind::Volume);
        let session = Session::new(loc.name.clone(), scan::start_scan(loc.path.clone()), space);
        self.sessions.insert(loc.path, session);
        self.hint_dismissed = false;
    }

    /// Selects (adding it to the sidebar when new) and scans a folder.
    pub fn scan_folder(&mut self, path: PathBuf) {
        let loc = self.known_location(&path).unwrap_or_else(|| {
            let loc = Location { name: platform::display_name(&path), path: path.clone(), kind: LocKind::Folder, favorite: false };
            self.recent.retain(|l| l.path != path);
            self.recent.insert(0, loc.clone());
            self.recent.truncate(6);
            loc
        });
        self.current = Some(loc);
        self.start_scan();
    }

    fn stop_scan(&mut self) {
        if let Some(s) = self.session() {
            s.scan.cancel();
        }
    }

    // ---- helpers ------------------------------------------------------------

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Path the item commands (reveal, info…) apply to.
    fn target_path(&self, id: Option<NodeId>) -> Option<PathBuf> {
        match self.session() {
            Some(s) => Some(s.path_of(id.unwrap_or_else(|| s.target()))),
            None => self.current.as_ref().map(|l| l.path.clone()),
        }
    }

    pub fn info_for(&self, id: Option<NodeId>) -> Option<InfoData> {
        if let Some(s) = self.session() {
            let id = id.unwrap_or_else(|| s.target());
            let tree = s.scan.tree.read();
            let n = tree.node(id);
            let path = tree.path(id);
            let kind = if n.is_package() {
                "Application or package"
            } else if n.is_dir() {
                "Folder"
            } else if n.flags & crate::tree::F_SYMLINK != 0 {
                "Alias or link"
            } else {
                "File"
            };
            return Some(InfoData {
                name: if id == tree.root() { s.name.clone() } else { n.display_name() },
                created: if self.opts.demo { n.mtime - 86_400 * 40 } else { platform::created_secs(&path) },
                modified: n.mtime,
                path,
                kind,
                size: Some(n.size),
                items: n.is_dir().then_some(n.items),
                flags: n.flags,
            });
        }
        let loc = self.current.as_ref()?;
        Some(InfoData {
            name: loc.name.clone(),
            path: loc.path.clone(),
            kind: if loc.kind == LocKind::Volume { "Volume" } else { "Folder" },
            size: None,
            items: None,
            created: platform::created_secs(&loc.path),
            modified: platform::modified_secs(&loc.path),
            flags: crate::tree::F_DIR,
        })
    }

    fn check_error_text(e: CheckError) -> &'static str {
        match e {
            CheckError::ParentSelected => t("This item is inside a folder that is already selected."),
            CheckError::Hidden => t("Hidden items are protected. You can allow removing them in Settings."),
            CheckError::System => t("This item is required by the system and cannot be selected."),
            CheckError::Empty => t("This file takes up no space on your disk. There is no need to remove it."),
            CheckError::Unscanned => t("This folder was not scanned completely and cannot be selected."),
        }
    }

    fn open_review(&mut self, kind: OpKind, only: Option<NodeId>) {
        if self.is_scanning() {
            self.toast(t("Wait until the scan is finished."));
            return;
        }
        let Some(s) = self.session() else { return };
        let items = s.op_items(only);
        if items.is_empty() {
            self.toast(t("Select items first by ticking their checkboxes."));
            return;
        }
        let dest = if kind == OpKind::Remove {
            None
        } else {
            let title = if kind == OpKind::Move { t("Select a folder to move the items to") } else { t("Select a folder to copy the items to") };
            match rfd::FileDialog::new().set_title(title).pick_folder() {
                Some(d) => Some(d),
                None => return,
            }
        };
        let tree = s.scan.tree.read();
        let review: Vec<ReviewItem> = items
            .into_iter()
            .map(|item| ReviewItem {
                name: tree.node(item.id).display_name(),
                parent: item.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                hidden: tree.in_hidden(item.id),
                item,
            })
            .collect();
        drop(tree);
        let session_id = s.id;
        let Some(path) = self.current.as_ref().map(|l| l.path.clone()) else { return };
        self.dialog = Dialog::Review { kind, items: review, dest, session: (path, session_id) };
    }

    /// Starts the reviewed operation.
    pub fn confirm_review(&mut self) {
        let Dialog::Review { kind, items, dest, session } = std::mem::replace(&mut self.dialog, Dialog::None) else { return };
        if self.opts.demo {
            // Demo data is not on disk: pretend everything worked.
            let results: Vec<ops::OpResult> =
                items.into_iter().map(|r| ops::OpResult { item: r.item, error: None, renamed: None }).collect();
            self.finish_operation(kind, &session, &results);
            return;
        }
        let items: Vec<ops::OpItem> = items.into_iter().map(|r| r.item).collect();
        let progress = ops::run(kind, items, dest);
        self.dialog = Dialog::Progress { progress, session };
    }

    fn finish_operation(&mut self, kind: OpKind, session: &(PathBuf, u64), results: &[ops::OpResult]) {
        // Node ids are only meaningful for the scan the items were reviewed on.
        if let Some(s) = self.sessions.get_mut(&session.0).filter(|s| s.id == session.1) {
            s.apply_results(results, kind != OpKind::Copy);
        }
        let name = |p: &Path| platform::display_name(p);
        self.dialog = Dialog::Summary(Summary {
            kind,
            ok: results.iter().filter(|r| r.error.is_none()).count(),
            size: results.iter().filter(|r| r.error.is_none()).map(|r| r.item.size).sum(),
            failed: results
                .iter()
                .filter_map(|r| r.error.as_ref().map(|e| (name(&r.item.path), e.clone())))
                .collect(),
            renamed: results
                .iter()
                .filter_map(|r| r.renamed.as_ref().map(|n| (name(&r.item.path), name(n))))
                .collect(),
        });
        self.volumes_checked = Instant::now() - Duration::from_secs(60);
    }

    fn export_csv(&mut self) {
        let Some(s) = self.session() else {
            self.toast(t("Scan a folder first."));
            return;
        };
        let default = format!("{} - disk report.csv", s.name);
        let Some(out) = rfd::FileDialog::new().set_file_name(default).add_filter("CSV", &["csv"]).save_file() else { return };
        let result = export::write_csv(&s.scan.tree.read(), s.view_root, &out);
        match result {
            Ok(rows) => self.toast(tf("Report exported: {} lines.", &[&util::fmt_count(rows)])),
            Err(e) => {
                self.dialog = Dialog::Message { title: t("The report could not be written").into(), body: e.to_string() };
            }
        }
    }

    fn add_favorite(&mut self, id: Option<NodeId>) {
        let Some(path) = self.target_path(id) else { return };
        if !path.is_dir() && !self.opts.demo {
            self.toast(t("Only folders can be added to Favorites."));
            return;
        }
        if self.settings.favorites.contains(&path) || self.known_location(&path).map(|l| !l.favorite && l.kind != LocKind::Folder).unwrap_or(false) {
            self.toast(t("This folder is already in the list."));
            return;
        }
        self.recent.retain(|l| l.path != path);
        self.settings.favorites.push(path);
        self.settings.save();
        self.toast(t("Folder added to Favorites."));
    }

    // ---- commands -----------------------------------------------------------

    pub fn run_cmd(&mut self, cmd: Cmd, id: Option<NodeId>, ctx: &egui::Context) {
        match cmd {
            Cmd::Scan => self.start_scan(),
            Cmd::ScanCustom => {
                if let Some(dir) = rfd::FileDialog::new().set_title(t("Select folder to scan")).pick_folder() {
                    self.scan_folder(dir);
                }
            }
            Cmd::Stop => self.stop_scan(),
            Cmd::ExportCsv => self.export_csv(),
            Cmd::AddFavorite => self.add_favorite(id),
            Cmd::Reveal => {
                if let Some(p) = self.target_path(id) {
                    if !self.opts.demo {
                        platform::reveal(&p);
                    }
                }
            }
            Cmd::QuickLook => {
                if let Some(p) = self.target_path(id) {
                    if !self.opts.demo {
                        platform::quick_look(&p);
                    }
                }
            }
            Cmd::Info => {
                if let Some(info) = self.info_for(id) {
                    self.dialog = Dialog::Info(info);
                }
            }
            Cmd::CopyPath => {
                if let Some(p) = self.target_path(id) {
                    ctx.copy_text(p.to_string_lossy().into_owned());
                    self.toast(t("Path copied."));
                }
            }
            Cmd::Find => {
                if let Some(s) = self.session_mut() {
                    s.search_open = !s.search_open;
                    if !s.search_open {
                        s.set_query(String::new());
                    }
                    s.mark_dirty();
                    self.focus_search = true;
                }
            }
            Cmd::ToggleSidebar => {
                self.settings.sidebar_open = !self.settings.sidebar_open;
                self.settings.save();
            }
            Cmd::ToggleChart => {
                self.settings.show_chart = !self.settings.show_chart;
                self.settings.save();
            }
            Cmd::TabOutline | Cmd::TabBiggest => {
                if let Some(s) = self.session_mut() {
                    s.tab = if cmd == Cmd::TabOutline { Tab::Outline } else { Tab::Biggest };
                    s.search_open = false;
                    s.mark_dirty();
                }
            }
            Cmd::GoBack | Cmd::GoForward => {
                if let Some(s) = self.session_mut() {
                    s.go_history(cmd == Cmd::GoForward);
                }
            }
            Cmd::GoParent => {
                if let Some(s) = self.session_mut() {
                    s.go_parent();
                }
            }
            Cmd::GoRoot => {
                if let Some(s) = self.session_mut() {
                    s.go_root();
                }
            }
            Cmd::Remove => self.open_review(OpKind::Remove, None),
            Cmd::Move => self.open_review(OpKind::Move, None),
            Cmd::Copy => self.open_review(OpKind::Copy, None),
            Cmd::DeselectAll => {
                if let Some(s) = self.session_mut() {
                    s.deselect_all();
                }
            }
            Cmd::Settings => {
                self.disk_access = self.opts.demo || platform::has_full_disk_access();
                self.dialog = Dialog::Settings;
            }
            Cmd::About => self.dialog = Dialog::About,
            Cmd::Help => self.dialog = Dialog::Help,
            Cmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        // Menus and shortcuts stay live under a modal dialog: nothing may change behind it.
        if !matches!(self.dialog, Dialog::None) && !matches!(action, Action::Hover(..)) {
            return;
        }
        let allow_hidden = self.settings.allow_removing_hidden;
        match action {
            Action::Cmd(c) => self.run_cmd(c, None, ctx),
            Action::On(c, id) => match c {
                Cmd::Remove | Cmd::Move | Cmd::Copy => {
                    let kind = match c {
                        Cmd::Move => OpKind::Move,
                        Cmd::Copy => OpKind::Copy,
                        _ => OpKind::Remove,
                    };
                    self.review_single(kind, id);
                }
                _ => self.run_cmd(c, Some(id), ctx),
            },
            Action::QuickRemove(id) => self.review_single(OpKind::Remove, id),
            Action::Open(id) => {
                if let Some(p) = self.target_path(Some(id)).filter(|_| !self.opts.demo) {
                    platform::open_file(&p);
                }
            }
            Action::SelectLocation(loc) => {
                self.current = Some(loc);
            }
            Action::RemoveFavorite(path) => {
                self.settings.favorites.retain(|p| p != &path);
                self.settings.save();
            }
            Action::SetOpMode(kind) => self.op_mode = kind,
            Action::Review => self.open_review(self.op_mode, None),
            Action::ShowDenied => {
                self.dialog = Dialog::Message {
                    title: t("Some folders were not scanned due to lack of access").into(),
                    body: if cfg!(target_os = "macos") {
                        t("Grant Full Disk Access to this app in System Settings → Privacy & Security, then scan again.").into()
                    } else {
                        t("Run the scan with an account that can read these folders to include them.").into()
                    },
                };
            }
            Action::DismissHint => self.hint_dismissed = true,
            Action::PaneWidth { flat, width, done } => {
                if flat {
                    self.settings.list_width = width;
                } else {
                    self.settings.column_width = width;
                }
                if done {
                    self.settings.save();
                }
            }
            other => {
                let mut error = None;
                let Some(s) = self.session_mut() else { return };
                match other {
                    Action::RowClick { col, id } => s.select(col, id),
                    Action::FlatClick(id) => {
                        s.focus = Some(id);
                    }
                    Action::SelectPath(id) => s.select_path(id),
                    Action::Step(step) => s.step(step),
                    Action::Hover(id, from_chart) => {
                        s.hover = Some(id);
                        s.hover_from_chart = from_chart;
                    }
                    Action::ToggleCheck(id) => error = s.toggle_check(id, allow_hidden).err(),
                    Action::ToggleGroup(dir, kind) => s.toggle_group(dir, kind),
                    Action::Crumb(id) => s.go_crumb(id),
                    Action::MoreBiggest => {
                        s.biggest_limit += 50;
                        s.mark_dirty();
                    }
                    Action::SetQuery(q) => s.set_query(q),
                    Action::CloseSearch => {
                        s.search_open = false;
                        s.set_query(String::new());
                        s.mark_dirty();
                    }
                    _ => {}
                }
                if let Some(e) = error {
                    self.toast(Self::check_error_text(e));
                }
            }
        }
    }

    /// Review for a single item from a context menu, with the selection rules applied.
    fn review_single(&mut self, kind: OpKind, id: NodeId) {
        let allow_hidden = self.settings.allow_removing_hidden;
        let Some(s) = self.session() else { return };
        let tree = s.scan.tree.read();
        let error = if tree.unscanned(id) {
            Some(CheckError::Unscanned)
        } else if kind == OpKind::Copy {
            None
        } else if !allow_hidden && tree.in_hidden(id) {
            Some(CheckError::Hidden)
        } else if id == tree.root() || ops::protected_reason(&tree.path(id)).is_some() {
            Some(CheckError::System)
        } else {
            None
        };
        drop(tree);
        match error {
            Some(e) => self.toast(Self::check_error_text(e)),
            None => self.open_review(kind, Some(id)),
        }
    }

    // ---- per-frame housekeeping -----------------------------------------------

    fn sync_appearance(&mut self, ctx: &egui::Context) {
        let dark = match self.settings.appearance {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::Auto => ctx.system_theme().map(|t| t == egui::Theme::Dark).unwrap_or(true),
        };
        if self.applied_dark != Some(dark) {
            self.pal = if dark { Palette::dark() } else { Palette::light() };
            theme::apply_visuals(ctx, &self.pal);
            self.applied_dark = Some(dark);
        }
        let lang = i18n::resolve(&self.settings.language);
        if self.applied_lang != Some(lang) {
            i18n::set_lang(lang);
            self.applied_lang = Some(lang);
            #[cfg(target_os = "macos")]
            {
                self.native_menu = Some(crate::menu::NativeMenu::install(ctx));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t("Sunburst Disk Analyser").into()));
        }
    }

    fn poll_operation(&mut self) {
        let Dialog::Progress { progress, session } = &self.dialog else { return };
        if !progress.finished.load(Ordering::Acquire) {
            return;
        }
        let (progress, session) = (progress.clone(), session.clone());
        let results = std::mem::take(&mut *progress.results.lock());
        self.finish_operation(progress.kind, &session, &results);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if matches!(self.dialog, Dialog::None) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.session().map(|s| s.search_open).unwrap_or(false) {
                self.actions.push(Action::CloseSearch);
            }
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        // Arrow keys walk the outline columns; the side buttons of a mouse go back and forward.
        let browsing = matches!(self.dialog, Dialog::None) && self.session().map(|s| s.tab == Tab::Outline && !s.search_open).unwrap_or(false);
        if browsing {
            let steps = [(Key::ArrowUp, Step::Up), (Key::ArrowDown, Step::Down), (Key::ArrowLeft, Step::Left), (Key::ArrowRight, Step::Right)];
            for (key, step) in steps {
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                    self.actions.push(Action::Step(step));
                }
            }
        }
        for (button, cmd) in [(egui::PointerButton::Extra1, Cmd::GoBack), (egui::PointerButton::Extra2, Cmd::GoForward)] {
            if ctx.input(|i| i.pointer.button_pressed(button)) {
                self.actions.push(Action::Cmd(cmd));
            }
        }
        // On macOS the native menu bar owns the shortcuts.
        if cfg!(target_os = "macos") {
            return;
        }
        let table = [
            (Modifiers::COMMAND, Key::R, Cmd::Scan),
            (Modifiers::COMMAND, Key::O, Cmd::ScanCustom),
            (Modifiers::COMMAND, Key::E, Cmd::ExportCsv),
            (Modifiers::COMMAND, Key::D, Cmd::AddFavorite),
            (Modifiers::COMMAND, Key::I, Cmd::Info),
            (Modifiers::COMMAND, Key::Y, Cmd::QuickLook),
            (Modifiers::COMMAND, Key::F, Cmd::Find),
            (Modifiers::COMMAND, Key::Num1, Cmd::TabOutline),
            (Modifiers::COMMAND, Key::Num2, Cmd::TabBiggest),
            (Modifiers::COMMAND, Key::OpenBracket, Cmd::GoBack),
            (Modifiers::COMMAND, Key::CloseBracket, Cmd::GoForward),
            (Modifiers::ALT, Key::ArrowLeft, Cmd::GoBack),
            (Modifiers::ALT, Key::ArrowRight, Cmd::GoForward),
            (Modifiers::COMMAND, Key::ArrowUp, Cmd::GoParent),
            (Modifiers::COMMAND, Key::Backspace, Cmd::Remove),
            (Modifiers::COMMAND, Key::Comma, Cmd::Settings),
            (Modifiers::COMMAND, Key::Q, Cmd::Quit),
        ];
        for (mods, key, cmd) in table {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key))) {
                self.actions.push(Action::Cmd(cmd));
            }
        }
    }

    /// A sideways two-finger swipe on a trackpad goes back or forward.
    fn handle_swipe(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let mut swiped = None;
        // The outline columns scroll sideways; a swipe counts where they cannot move further.
        let strip = crate::views::strip_scroll(ctx);
        ctx.input(|i| {
            let over = i.pointer.hover_pos().map(|p| strip.rect.contains(p)).unwrap_or(false);
            let scrolls = |right: bool| over && if right { strip.offset > 0.5 } else { strip.offset < strip.end - 0.5 };
            for e in &i.events {
                if let egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers } = e {
                    if modifiers.is_none() {
                        self.trackpad_at = now;
                        swiped = self.swipe.feed(now, delta.x, delta.y, scrolls).or(swiped);
                    }
                }
            }
        });
        if now - self.trackpad_at < 0.3 && !self.swipe.scrolling() {
            // Sideways travel of a swipe must not also scroll the columns,
            // not even with what egui spreads over the next frames.
            ctx.input_mut(|i| i.smooth_scroll_delta.x = 0.0);
        }
        if let Some(forward) = swiped {
            self.actions.push(Action::Cmd(if forward { Cmd::GoForward } else { Cmd::GoBack }));
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !matches!(self.dialog, Dialog::None) || self.opts.demo {
            return;
        }
        if let Some(path) = dropped.into_iter().next() {
            let dir = if path.is_dir() { Some(path) } else { path.parent().map(Path::to_path_buf) };
            if let Some(dir) = dir {
                self.scan_folder(dir);
            }
        }
    }

    /// Scripted scenes used to render the README screenshots.
    fn drive_screenshot(&mut self, ctx: &egui::Context) {
        let Some((scene, out)) = self.opts.shot.clone() else { return };
        ctx.request_repaint();
        if self.frame == 4 {
            let allow = true;
            match scene.as_str() {
                "idle" => {
                    self.sessions.clear();
                }
                "columns" | "review" | "selection" => {
                    if let Some(s) = self.session_mut() {
                        s.select(0, 1);
                        s.refresh();
                        let second = match s.columns.get(1).and_then(|c| c.items.first()) {
                            Some(crate::session::Item::Row(r)) => Some(r.id),
                            _ => None,
                        };
                        if scene != "columns" {
                            let _ = s.toggle_check(2, allow);
                            let _ = s.toggle_check(3, allow);
                            if let Some(id) = second {
                                let _ = s.toggle_check(id, allow);
                            }
                        } else if let Some(id) = second {
                            s.hover = Some(id);
                        }
                    }
                    self.hover_lock = scene == "columns";
                    if scene == "review" {
                        self.open_review(OpKind::Remove, None);
                    }
                }
                "biggest" => {
                    if let Some(s) = self.session_mut() {
                        s.tab = Tab::Biggest;
                        s.mark_dirty();
                    }
                }
                "hidden" => {
                    if let Some(s) = self.session_mut() {
                        s.toggle_group(0, GroupKind::Hidden);
                        s.hover = Some(1);
                    }
                    self.hover_lock = true;
                }
                "settings" => self.dialog = Dialog::Settings,
                _ => {
                    if let Some(s) = self.session_mut() {
                        s.hover = Some(3);
                    }
                    self.hover_lock = true;
                }
            }
        }
        if self.frame == 14 && !self.shot_requested {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let [w, h] = img.size;
            let saved = image::RgbaImage::from_raw(w as u32, h as u32, img.as_raw().to_vec())
                .ok_or_else(|| "bad screenshot buffer".to_string())
                .and_then(|i| i.save(&out).map_err(|e| e.to_string()));
            if let Err(e) = saved {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.pal.bg_bottom;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.frame += 1;
        self.sync_appearance(ctx);
        // The system puts the window buttons back where it wants them on every resize.
        #[cfg(target_os = "macos")]
        {
            let (x, y) = crate::views::window_buttons_at();
            self.lights = platform::place_window_buttons(frame, x, y).unwrap_or(0.0);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = frame;
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.native_menu {
            for cmd in menu.poll() {
                // ⌘⌫ belongs to the text field while one is being edited.
                if cmd == Cmd::Remove && ctx.wants_keyboard_input() {
                    continue;
                }
                self.actions.push(Action::Cmd(cmd));
            }
            menu.sync(self.settings.sidebar_open, self.settings.show_chart);
        }
        self.handle_shortcuts(ctx);
        self.handle_swipe(ctx);
        self.handle_drops(ctx);
        self.poll_operation();

        if !self.opts.demo && self.volumes_checked.elapsed() > Duration::from_secs(20) {
            self.volumes = platform::volumes();
            self.volumes_checked = Instant::now();
        }

        let mut busy = matches!(self.dialog, Dialog::Progress { .. });
        if let Some(s) = self.session_mut() {
            busy |= s.refresh();
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        if let Some((_, since)) = &self.toast {
            if since.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        // The preview pane shows a thumbnail of the selected file.
        self.thumbs.poll(ctx);
        let previewed = self.session().and_then(|s| s.selected_file().map(|id| s.path_of(id)));
        if let Some(path) = previewed.filter(|_| !self.opts.demo) {
            self.thumbs.want(ctx, &path);
        }

        let mut actions = std::mem::take(&mut self.actions);
        self.draw(ctx, &mut actions);
        self.draw_dialog(ctx);
        actions.append(&mut self.actions);

        // Hover is re-established by the views on every frame.
        let had_hover = self.session().and_then(|s| s.hover);
        if !self.hover_lock {
            if let Some(s) = self.session_mut() {
                s.hover = None;
            }
        }
        for action in actions {
            self.apply(action, ctx);
        }
        if self.session().and_then(|s| s.hover) != had_hover {
            ctx.request_repaint();
        }
        self.drive_screenshot(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Item;
    use egui::{pos2, Event, PointerButton, Pos2};

    /// The app on demo data, driven frame by frame without a window.
    struct Bench {
        app: App,
        ctx: egui::Context,
        time: f64,
    }

    impl Bench {
        fn new() -> Bench {
            let ctx = egui::Context::default();
            let app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), Options { demo: true, ..Options::default() });
            let mut bench = Bench { app, ctx, time: 0.0 };
            bench.frames(3, vec![]);
            bench
        }

        fn frames(&mut self, count: usize, events: Vec<Event>) {
            let mut events = Some(events);
            for _ in 0..count {
                self.time += 1.0 / 60.0;
                let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
                let input = egui::RawInput { screen_rect: Some(screen), time: Some(self.time), events: events.take().unwrap_or_default(), ..Default::default() };
                let (app, ctx) = (&mut self.app, self.ctx.clone());
                let _ = ctx.run(input, |ctx| {
                    if let Some(s) = app.session_mut() {
                        s.refresh();
                    }
                    let mut actions = Vec::new();
                    app.draw(ctx, &mut actions);
                    for action in actions {
                        app.apply(action, ctx);
                    }
                });
            }
        }

        fn button(&mut self, pos: Pos2, pressed: bool) {
            self.frames(1, vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() }]);
        }

        fn drag(&mut self, from: Pos2, by: f32) {
            self.frames(2, vec![Event::PointerMoved(from)]);
            self.button(from, true);
            for step in 1..=8 {
                self.frames(1, vec![Event::PointerMoved(pos2(from.x + by * step as f32 / 8.0, from.y))]);
            }
            self.button(pos2(from.x + by, from.y), false);
            self.frames(2, vec![]);
        }

        fn strip(&self) -> egui::Rect {
            crate::views::strip_scroll(&self.ctx).rect
        }
    }

    #[test]
    fn dragging_a_column_edge_resizes_the_columns() {
        let mut bench = Bench::new();
        let strip = bench.strip();
        assert_eq!(strip.width(), 264.0, "one column of the standard width");

        // The edge follows the pointer, wherever on the grip it was taken.
        bench.drag(pos2(strip.right() + 3.0, 400.0), 40.0);
        assert_eq!(bench.app.settings.column_width, 304.0);
        assert_eq!(bench.strip().width(), 304.0);

        // With two columns in sight, their common edge with the chart moves both.
        let first = bench.app.session().unwrap().columns[0].items.iter().find_map(|i| if let Item::Row(r) = i { Some(r.id) } else { None }).unwrap();
        bench.app.apply(Action::RowClick { col: 0, id: first }, &bench.ctx.clone());
        bench.frames(30, vec![]);
        let strip = bench.strip();
        assert_eq!(strip.width(), 608.0);
        bench.drag(pos2(strip.right(), 400.0), -80.0);
        assert_eq!(bench.app.settings.column_width, 264.0);
        // So does the edge between them.
        bench.drag(pos2(bench.strip().left() + 264.0, 400.0), -64.0);
        assert_eq!(bench.app.settings.column_width, 200.0);
        assert_eq!(bench.strip().width(), 400.0);

        // Neither the columns nor the chart can be squeezed out.
        bench.drag(pos2(bench.strip().left() + 200.0, 400.0), -190.0);
        assert_eq!(bench.app.settings.column_width, 150.0);
        bench.drag(pos2(bench.strip().right(), 400.0), 900.0);
        let strip = bench.strip();
        assert!(strip.width() > 608.0 && 1200.0 - strip.right() > 300.0, "{strip:?}");

        // A double-click on an edge brings the standard width back.
        let edge = pos2(strip.right(), 400.0);
        for pressed in [true, false, true, false] {
            bench.button(edge, pressed);
        }
        assert_eq!(bench.app.settings.column_width, 264.0);
    }
}
