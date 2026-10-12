//! Native macOS menu bar. Other platforms use an in-window menu bar.

use crate::app::Cmd;
use crate::i18n::t;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

static EVENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

const COMMANDS: &[(&str, Cmd)] = &[
    ("scan", Cmd::Scan),
    ("scan-custom", Cmd::ScanCustom),
    ("stop", Cmd::Stop),
    ("export", Cmd::ExportCsv),
    ("favorite", Cmd::AddFavorite),
    ("reveal", Cmd::Reveal),
    ("info", Cmd::Info),
    ("quicklook", Cmd::QuickLook),
    ("copy-path", Cmd::CopyPath),
    ("find", Cmd::Find),
    ("sidebar", Cmd::ToggleSidebar),
    ("outline", Cmd::TabOutline),
    ("biggest", Cmd::TabBiggest),
    ("chart", Cmd::ToggleChart),
    ("back", Cmd::GoBack),
    ("forward", Cmd::GoForward),
    ("parent", Cmd::GoParent),
    ("root", Cmd::GoRoot),
    ("remove", Cmd::Remove),
    ("move", Cmd::Move),
    ("copy", Cmd::Copy),
    ("deselect", Cmd::DeselectAll),
    ("settings", Cmd::Settings),
    ("about", Cmd::About),
    ("help", Cmd::Help),
];

pub struct NativeMenu {
    _menu: Menu,
    sidebar: MenuItem,
    chart: MenuItem,
    state: Cell<Option<(bool, bool)>>,
}

fn key(code: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(Modifiers::SUPER), code))
}

fn item(id: &str, text: &str, accel: Option<Accelerator>) -> MenuItem {
    MenuItem::with_id(id, text, true, accel)
}

impl NativeMenu {
    /// Builds the menu bar in the current language and makes it the application menu.
    pub fn install(ctx: &egui::Context) -> NativeMenu {
        if EVENTS.set(Mutex::new(Vec::new())).is_ok() {
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Some(q) = EVENTS.get() {
                    q.lock().unwrap().push(e.id().as_ref().to_string());
                }
                ctx.request_repaint();
            }));
        }
        let app_name = t("Sunburst Disk Analyser");
        let sep = PredefinedMenuItem::separator;
        let menu = Menu::new();

        let app = Submenu::new(app_name, true);
        let _ = app.append_items(&[
            &item("about", t("About Sunburst Disk Analyser"), None),
            &sep(),
            &item("settings", t("Settings…"), key(Code::Comma)),
            &sep(),
            &PredefinedMenuItem::services(None),
            &sep(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &sep(),
            &PredefinedMenuItem::quit(None),
        ]);

        let file = Submenu::new(t("File"), true);
        let _ = file.append_items(&[
            &item("scan", t("Scan"), key(Code::KeyR)),
            &item("scan-custom", t("Scan Custom Folder…"), key(Code::KeyO)),
            &item("stop", t("Stop Scanning"), key(Code::Period)),
            &sep(),
            &item("export", t("Export Report to CSV…"), key(Code::KeyE)),
            &sep(),
            &item("favorite", t("Add Folder to Favorites"), key(Code::KeyD)),
            &sep(),
            &item("reveal", t("Show in Finder"), None),
            &item("info", t("Get Information"), key(Code::KeyI)),
            &item("quicklook", t("Quick Look"), key(Code::KeyY)),
            &item("copy-path", t("Copy Path"), None),
            &sep(),
            &PredefinedMenuItem::close_window(None),
        ]);

        let edit = Submenu::new(t("Edit"), true);
        let _ = edit.append_items(&[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &sep(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
            &sep(),
            &item("find", t("Find"), key(Code::KeyF)),
        ]);

        let sidebar = item("sidebar", t("Collapse Category List"), None);
        let chart = item("chart", t("Hide Chart"), None);
        let view = Submenu::new(t("View"), true);
        let _ = view.append_items(&[
            &sidebar,
            &sep(),
            &item("outline", t("Folders Outline"), key(Code::Digit1)),
            &item("biggest", t("Biggest Files"), key(Code::Digit2)),
            &sep(),
            &chart,
            &sep(),
            &item("back", t("Back"), key(Code::BracketLeft)),
            &item("forward", t("Forward"), key(Code::BracketRight)),
            &item("parent", t("Go to Parent Folder"), key(Code::ArrowUp)),
            &item("root", t("Go to Root Folder"), None),
            &sep(),
            &PredefinedMenuItem::fullscreen(None),
        ]);

        let selected = Submenu::new(t("Selected"), true);
        let _ = selected.append_items(&[
            &item("remove", t("Remove…"), key(Code::Backspace)),
            &item("move", t("Move to Folder…"), None),
            &item("copy", t("Copy to Folder…"), None),
            &sep(),
            &item("deselect", t("Deselect All"), None),
        ]);

        let window = Submenu::new(t("Window"), true);
        let _ = window.append_items(&[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
            &sep(),
            &PredefinedMenuItem::bring_all_to_front(None),
        ]);

        let help = Submenu::new(t("Help"), true);
        let _ = help.append_items(&[&item("help", t("Sunburst Disk Analyser Help"), None)]);

        let _ = menu.append_items(&[&app, &file, &edit, &view, &selected, &window, &help]);
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();
        NativeMenu { _menu: menu, sidebar, chart, state: Cell::new(None) }
    }

    /// Commands chosen in the menu since the last call.
    pub fn poll(&self) -> Vec<Cmd> {
        let Some(q) = EVENTS.get() else { return Vec::new() };
        let ids = std::mem::take(&mut *q.lock().unwrap());
        ids.iter().filter_map(|id| COMMANDS.iter().find(|(k, _)| k == id).map(|(_, c)| *c)).collect()
    }

    /// Keeps the titles of the toggle items in line with the window state.
    pub fn sync(&self, sidebar_open: bool, chart_shown: bool) {
        if self.state.get() == Some((sidebar_open, chart_shown)) {
            return;
        }
        self.state.set(Some((sidebar_open, chart_shown)));
        self.sidebar.set_text(if sidebar_open { t("Collapse Category List") } else { t("Expand Category List") });
        self.chart.set_text(if chart_shown { t("Hide Chart") } else { t("Show Chart") });
    }
}
