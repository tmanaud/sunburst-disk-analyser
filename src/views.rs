//! Painting of the main window: sidebar, toolbar, outline columns, flat
//! lists, sunburst chart, breadcrumbs and footer.

use crate::app::{Action, App, Cmd};
use crate::chart::{self, Geo, Hit, SegKind};
use crate::i18n::{t, tf, tn};
use crate::ops::OpKind;
use crate::platform::{self, LocKind, Location};
use crate::session::{Check, Column, GroupKind, Item, Row, Session, Tab};
use crate::theme::{regular, semibold};
use crate::tree::{NodeId, F_DIR};
use crate::widgets::{self, Btn};
use crate::{icons, util};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Sense, Shape, Stroke, Ui};

const PAD: f32 = 8.0;
const SIDEBAR_W: f32 = 210.0;
const TOPBAR_H: f32 = 56.0;
const FOOTER_H: f32 = 74.0;
const CRUMB_H: f32 = 30.0;
const ROW_H: f32 = 32.0;
const FLAT_ROW_H: f32 = 46.0;
const COL_W: f32 = 264.0;
const FLAT_W: f32 = 320.0;
/// Room left of the toolbar for the macOS window buttons.
const TRAFFIC_LIGHTS: f32 = 72.0;

fn mac() -> bool {
    cfg!(target_os = "macos")
}

impl App {
    pub fn draw(&mut self, ctx: &egui::Context, acts: &mut Vec<Action>) {
        let focus_search = std::mem::take(&mut self.focus_search);
        let app: &App = self;
        if !mac() {
            app.menu_bar(ctx, acts);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(app.pal.bar)).show(ctx, |ui| {
            let area = ui.max_rect().shrink(PAD);
            let main = if app.settings.sidebar_open {
                let side = Rect::from_min_size(area.min, vec2(SIDEBAR_W, area.height()));
                app.sidebar(ui, side, acts);
                Rect::from_min_max(pos2(side.right() + PAD, area.top()), area.max)
            } else {
                area
            };
            app.main_panel(ui, main, acts, focus_search);
            app.paint_toast(ui, main);
        });
    }

    // ---- menu bar (Windows and Linux) ------------------------------------------

    fn menu_bar(&self, ctx: &egui::Context, acts: &mut Vec<Action>) {
        let frame = egui::Frame::NONE.fill(self.pal.bar).inner_margin(egui::Margin::symmetric(8, 3));
        egui::TopBottomPanel::top("menu-bar").frame(frame).show(ctx, |ui| {
            let mut entry = |ui: &mut Ui, label: &str, keys: &str, cmd: Cmd| {
                if ui.add(egui::Button::new(label).shortcut_text(keys)).clicked() {
                    acts.push(Action::Cmd(cmd));
                    ui.close_menu();
                }
            };
            egui::menu::bar(ui, |ui| {
                ui.menu_button(t("File"), |ui| {
                    entry(ui, t("Scan"), "Ctrl+R", Cmd::Scan);
                    entry(ui, t("Scan Custom Folder…"), "Ctrl+O", Cmd::ScanCustom);
                    entry(ui, t("Stop Scanning"), "", Cmd::Stop);
                    ui.separator();
                    entry(ui, t("Export Report to CSV…"), "Ctrl+E", Cmd::ExportCsv);
                    ui.separator();
                    entry(ui, t("Add Folder to Favorites"), "Ctrl+D", Cmd::AddFavorite);
                    ui.separator();
                    entry(ui, reveal_label(), "", Cmd::Reveal);
                    entry(ui, t("Get Information"), "Ctrl+I", Cmd::Info);
                    entry(ui, t(platform::preview_label()), "Ctrl+Y", Cmd::QuickLook);
                    entry(ui, t("Copy Path"), "", Cmd::CopyPath);
                    ui.separator();
                    entry(ui, t("Settings…"), "Ctrl+,", Cmd::Settings);
                    entry(ui, t("Quit"), "Ctrl+Q", Cmd::Quit);
                });
                ui.menu_button(t("Edit"), |ui| {
                    entry(ui, t("Find"), "Ctrl+F", Cmd::Find);
                });
                ui.menu_button(t("View"), |ui| {
                    let sidebar = if self.settings.sidebar_open { t("Collapse Category List") } else { t("Expand Category List") };
                    entry(ui, sidebar, "", Cmd::ToggleSidebar);
                    ui.separator();
                    entry(ui, t("Folders Outline"), "Ctrl+1", Cmd::TabOutline);
                    entry(ui, t("Biggest Files"), "Ctrl+2", Cmd::TabBiggest);
                    ui.separator();
                    entry(ui, if self.settings.show_chart { t("Hide Chart") } else { t("Show Chart") }, "", Cmd::ToggleChart);
                    ui.separator();
                    entry(ui, t("Go to Parent Folder"), "Ctrl+↑", Cmd::GoParent);
                    entry(ui, t("Go to Root Folder"), "", Cmd::GoRoot);
                });
                ui.menu_button(t("Selected"), |ui| {
                    entry(ui, t("Remove…"), "Ctrl+⌫", Cmd::Remove);
                    entry(ui, t("Move to Folder…"), "", Cmd::Move);
                    entry(ui, t("Copy to Folder…"), "", Cmd::Copy);
                    ui.separator();
                    entry(ui, t("Deselect All"), "", Cmd::DeselectAll);
                });
                ui.menu_button(t("Help"), |ui| {
                    entry(ui, t("Sunburst Disk Analyser Help"), "", Cmd::Help);
                    entry(ui, t("About Sunburst Disk Analyser"), "", Cmd::About);
                });
            });
        });
    }

    /// Lets the window be dragged by an empty toolbar area (macOS has no title bar).
    fn drag_region(&self, ui: &mut Ui, rect: Rect, key: &str) {
        if !mac() {
            return;
        }
        let resp = ui.interact(rect, ui.id().with(("drag", key)), Sense::click_and_drag());
        if resp.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }

    /// The two round toggles (category list, chart). Returns the x after them.
    fn panel_toggles(&self, ui: &mut Ui, x: f32, y: f32, acts: &mut Vec<Action>) -> f32 {
        let pal = &self.pal;
        let (resp, icon) = widgets::round_button(ui, pos2(x + 14.0, y), 14.0, "sidebar", pal, false);
        icons::sidebar(ui.painter(), icon, pal.text_dim);
        let tip = if self.settings.sidebar_open { t("Collapse Category List") } else { t("Expand Category List") };
        if resp.on_hover_text(tip).clicked() {
            acts.push(Action::Cmd(Cmd::ToggleSidebar));
        }
        let (resp, icon) = widgets::round_button(ui, pos2(x + 48.0, y), 14.0, "chart", pal, false);
        icons::chart(ui.painter(), icon, if self.settings.show_chart { pal.text } else { pal.text_faint });
        let tip = if self.settings.show_chart { t("Hide Chart") } else { t("Show Chart") };
        if resp.on_hover_text(tip).clicked() {
            acts.push(Action::Cmd(Cmd::ToggleChart));
        }
        x + 68.0
    }

    // ---- sidebar ---------------------------------------------------------------

    fn sidebar(&self, ui: &mut Ui, rect: Rect, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        widgets::gradient_panel(ui.painter(), rect, 14.0, pal.side_top, pal.side_bottom, pal.panel_line);
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), 44.0));
        self.drag_region(ui, top, "side");
        let x = if mac() { rect.left() + TRAFFIC_LIGHTS + 4.0 } else { rect.left() + 12.0 };
        self.panel_toggles(ui, x, rect.top() + if mac() { 14.0 } else { 24.0 }, acts);

        let list = Rect::from_min_max(pos2(rect.left(), rect.top() + 46.0), pos2(rect.right(), rect.bottom() - 52.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list).id_salt("sidebar-list"));
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::ScrollArea::vertical().id_salt("sidebar-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
            if let Some(home) = &self.home {
                self.sidebar_row(ui, home, acts);
            }
            if !self.volumes.is_empty() {
                self.sidebar_header(ui, t("Storages"));
                for (loc, _) in &self.volumes {
                    self.sidebar_row(ui, loc, acts);
                }
            }
            let favorites = self.favorites();
            if !self.folders.is_empty() || !favorites.is_empty() || !self.recent.is_empty() {
                self.sidebar_header(ui, t("Folders"));
                for loc in self.folders.iter().chain(favorites.iter()).chain(self.recent.iter()) {
                    self.sidebar_row(ui, loc, acts);
                }
            }
            ui.add_space(10.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
            let b = r.shrink2(vec2(8.0, 1.0));
            let resp = widgets::button(ui, b, "", Btn::Outline, true, pal, regular(13.5));
            let icon = Rect::from_center_size(pos2(b.left() + 24.0, b.center().y), vec2(18.0, 18.0));
            icons::plus_circle(ui.painter(), icon, pal.text);
            let label = widgets::fit_end(ui, t("Choose Folder…"), &regular(13.5), b.width() - 54.0);
            ui.painter().text(pos2(b.left() + 44.0, b.center().y), Align2::LEFT_CENTER, label, regular(13.5), pal.text);
            if resp.on_hover_text(t("Choose a new folder to scan")).clicked() {
                acts.push(Action::Cmd(Cmd::ScanCustom));
            }
            ui.add_space(8.0);
        });

        // Settings entry pinned at the bottom.
        let row = Rect::from_min_max(pos2(rect.left() + 8.0, rect.bottom() - 46.0), pos2(rect.right() - 8.0, rect.bottom() - 8.0));
        let resp = ui.interact(row, ui.id().with("side-settings"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(row, CornerRadius::same(9), pal.row_hover);
        }
        icons::gear(ui.painter(), Rect::from_center_size(pos2(row.left() + 24.0, row.center().y), vec2(17.0, 17.0)), pal.text_dim);
        ui.painter().text(pos2(row.left() + 44.0, row.center().y), Align2::LEFT_CENTER, t("Settings"), regular(13.0), pal.text_dim);
        if resp.clicked() {
            acts.push(Action::Cmd(Cmd::Settings));
        }
    }

    fn sidebar_header(&self, ui: &mut Ui, text: &str) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
        ui.painter().text(pos2(r.left() + 20.0, r.center().y + 3.0), Align2::LEFT_CENTER, text, regular(12.0), self.pal.text_dim);
    }

    fn sidebar_row(&self, ui: &mut Ui, loc: &Location, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
        let r = outer.shrink2(vec2(8.0, 2.0));
        let selected = self.current.as_ref().map(|c| c.path == loc.path).unwrap_or(false);
        let p = ui.painter();
        if selected {
            p.rect_filled(r, CornerRadius::same(9), pal.side_selected);
            p.rect_stroke(r, CornerRadius::same(9), Stroke::new(1.0_f32, pal.control_line), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(r, CornerRadius::same(9), pal.row_hover);
        }
        let ink = if selected || pal.dark { pal.text } else { pal.text_dim };
        let glyph = if pal.dark { Color32::from_rgb(196, 214, 242) } else { Color32::from_rgb(70, 100, 150) };
        let glyph = if selected && pal.dark { Color32::WHITE } else { glyph };
        let hole = if selected { pal.side_selected } else { pal.side_bottom };
        let icon = Rect::from_center_size(pos2(r.left() + 24.0, r.center().y), vec2(20.0, 20.0));
        match loc.kind {
            LocKind::Home => icons::home(p, icon, glyph),
            LocKind::Volume => icons::drive(p, icon, glyph, hole),
            LocKind::Desktop => icons::desktop(p, icon, glyph),
            LocKind::Pictures => icons::camera(p, icon, glyph, hole),
            LocKind::Documents => icons::document(p, icon, glyph, hole),
            LocKind::Downloads => icons::download(p, icon, glyph),
            LocKind::Folder if loc.favorite => icons::star(p, icon, glyph),
            LocKind::Folder => icons::folder(p, icon, glyph, glyph.gamma_multiply(0.8)),
        }
        let status = self.sessions.get(&loc.path).map(|s| s.scan.is_done());
        let text_w = r.width() - 44.0 - if status.is_some() { 30.0 } else { 8.0 };
        let name = widgets::fit_end(ui, &loc.name, &regular(14.0), text_w);
        let truncated = name != loc.name;
        ui.painter().text(pos2(r.left() + 44.0, r.center().y), Align2::LEFT_CENTER, name, regular(14.0), ink);
        let mark = Rect::from_center_size(pos2(r.right() - 18.0, r.center().y), vec2(15.0, 15.0));
        match status {
            Some(true) => icons::check(ui.painter(), mark, ink, 1.8),
            Some(false) => widgets::spinner(ui.painter(), mark.center(), 6.5, ink, ui.input(|i| i.time)),
            None => {}
        }
        if resp.clicked() {
            acts.push(Action::SelectLocation(loc.clone()));
        }
        resp.context_menu(|ui| {
            if ui.button(reveal_label()).clicked() {
                if !self.opts.demo {
                    platform::reveal(&loc.path);
                }
                ui.close_menu();
            }
            if loc.favorite && ui.button(t("Remove from Favorites")).clicked() {
                acts.push(Action::RemoveFavorite(loc.path.clone()));
                ui.close_menu();
            }
        });
        if truncated {
            resp.on_hover_text(&loc.name);
        }
    }

    // ---- main panel ------------------------------------------------------------

    fn main_panel(&self, ui: &mut Ui, rect: Rect, acts: &mut Vec<Action>, focus_search: bool) {
        let pal = &self.pal;
        widgets::gradient_panel(ui.painter(), rect, 14.0, pal.bg_top, pal.bg_bottom, pal.panel_line);
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), TOPBAR_H));
        let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FOOTER_H), rect.max);
        let line = Stroke::new(1.0_f32, pal.panel_line.gamma_multiply(0.7));
        ui.painter().line_segment([top.left_bottom(), top.right_bottom()], line);
        ui.painter().line_segment([footer.left_top(), footer.right_top()], line);

        self.drag_region(ui, top, "top");
        let session = self.session();
        self.topbar(ui, top, session, acts, focus_search);
        match session {
            Some(s) => {
                let crumbs = Rect::from_min_max(pos2(rect.left() + 1.0, footer.top() - CRUMB_H), pos2(rect.right() - 1.0, footer.top()));
                let content = Rect::from_min_max(pos2(rect.left() + 1.0, top.bottom() + 1.0), pos2(rect.right() - 1.0, crumbs.top()));
                self.content(ui, content, s, acts);
                self.crumbs(ui, crumbs, s, acts);
            }
            None => {
                let content = Rect::from_min_max(pos2(rect.left(), top.bottom()), pos2(rect.right(), footer.top()));
                self.idle(ui, content);
            }
        }
        self.footer(ui, footer, session, acts);
    }

    fn topbar(&self, ui: &mut Ui, rect: Rect, session: Option<&Session>, acts: &mut Vec<Action>, focus_search: bool) {
        let pal = &self.pal;
        let cy = rect.center().y;
        let mut x = rect.left() + 14.0;
        if !self.settings.sidebar_open {
            if mac() {
                x += TRAFFIC_LIGHTS;
            }
            x = self.panel_toggles(ui, x, cy, acts) + 6.0;
        }

        // Right side: help, and rescan once a scan is complete.
        let mut right = rect.right() - 14.0;
        let (resp, icon) = widgets::round_button(ui, pos2(right - 14.0, cy), 14.0, "help", pal, false);
        icons::help(ui.painter(), icon, pal.text_dim, semibold(15.0));
        if resp.on_hover_text(t("Help")).clicked() {
            acts.push(Action::Cmd(Cmd::Help));
        }
        right -= 36.0;

        let Some(s) = session else {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, t("Sunburst Disk Analyser"), semibold(15.5), pal.text);
            return;
        };

        if s.scan.is_done() {
            let (resp, icon) = widgets::round_button(ui, pos2(right - 14.0, cy), 14.0, "rescan", pal, false);
            icons::refresh(ui.painter(), icon, pal.text_dim);
            if resp.on_hover_text(t("Scan again")).clicked() {
                acts.push(Action::Cmd(Cmd::Scan));
            }
            right -= 36.0;
        }

        let (resp, icon) = widgets::round_button(ui, pos2(x + 14.0, cy), 14.0, "search", pal, s.search_open);
        icons::search(ui.painter(), icon, pal.text);
        if resp.on_hover_text(t("Search by name")).clicked() {
            acts.push(Action::Cmd(Cmd::Find));
        }
        x += 38.0;

        if s.search_open {
            let field = Rect::from_min_size(pos2(x, cy - 16.0), vec2(280.0f32.min(right - x - 20.0), 32.0));
            ui.painter().rect_filled(field, CornerRadius::same(16), pal.control.gamma_multiply(0.75));
            ui.painter().rect_stroke(field, CornerRadius::same(16), Stroke::new(1.0_f32, pal.control_line), egui::StrokeKind::Inside);
            let mut query = s.query.clone();
            let edit = egui::TextEdit::singleline(&mut query)
                .hint_text(tf("Search in {}", &[&s.name]))
                .frame(false)
                .font(regular(13.5))
                .id(egui::Id::new("search-field"));
            let resp = ui.put(field.shrink2(vec2(14.0, 5.0)), edit);
            if focus_search {
                resp.request_focus();
            }
            if query != s.query {
                acts.push(Action::SetQuery(query));
            }
            x = field.right() + 16.0;
        } else {
            let labels = [t("Folders Outline"), t("Biggest Files")];
            let widest = labels.iter().map(|l| widgets::text_width(ui, l, &semibold(13.5))).fold(0.0, f32::max);
            let want = ((widest + 34.0) * 2.0).max(270.0);
            let seg = Rect::from_min_size(pos2(x, cy - 17.0), vec2(want.min(right - x - 20.0), 34.0));
            let selected = if s.tab == Tab::Outline { 0 } else { 1 };
            match widgets::segmented(ui, seg, &labels, selected, pal) {
                Some(0) => acts.push(Action::Cmd(Cmd::TabOutline)),
                Some(1) => acts.push(Action::Cmd(Cmd::TabBiggest)),
                _ => {}
            }
            x = seg.right() + 16.0;
        }

        // Title: name and size of the scanned location, centred in the space left.
        let space = right - x - 8.0;
        if space < 60.0 {
            return;
        }
        let scanning = !s.scan.is_done();
        let size = format!("{} {}", t("Size:"), util::fmt_size(s.total_size));
        let size_w = widgets::text_width(ui, &size, &regular(14.0));
        let name = widgets::fit_middle(ui, &s.name, &regular(14.0), (space - size_w - 40.0).max(30.0));
        let name_w = widgets::text_width(ui, &name, &regular(14.0));
        let total = name_w + 12.0 + size_w + if scanning { 24.0 } else { 0.0 };
        let mut tx = (x + (space - total) * 0.5).max(x);
        if scanning {
            widgets::spinner(ui.painter(), pos2(tx + 7.0, cy), 6.5, pal.text_dim, ui.input(|i| i.time));
            tx += 24.0;
        }
        ui.painter().text(pos2(tx, cy), Align2::LEFT_CENTER, name, regular(14.0), pal.text_dim);
        if space >= total {
            ui.painter().text(pos2(tx + name_w + 12.0, cy), Align2::LEFT_CENTER, size, regular(14.0), pal.text_dim);
        }
    }

    // ---- start screen ------------------------------------------------------------

    fn idle(&self, ui: &mut Ui, rect: Rect) {
        let pal = &self.pal;
        let Some(loc) = &self.current else {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, t("Choose a folder to scan"), regular(15.0), pal.text_dim);
            return;
        };
        let p = ui.painter_at(rect);
        let art = rect.width() > 760.0;
        if art {
            let radius = (rect.height() * 0.3).min(rect.width() * 0.17);
            let c = pos2(rect.right() - radius * 1.45 - 16.0, rect.center().y);
            chart::paint_idle_art(&p, c, radius, pal);
            let label = widgets::fit_end(ui, &loc.name, &regular(15.0), radius * 0.8);
            p.text(c, Align2::CENTER_CENTER, label, regular(15.0), Color32::WHITE);
        }
        let block_w = 420.0f32.min(rect.width() - 40.0);
        let left = if art { rect.left() + (rect.width() * 0.56 - block_w).max(20.0) * 0.5 + 10.0 } else { rect.center().x - block_w * 0.5 };
        let cy = rect.center().y - 20.0;
        let icon = Rect::from_center_size(pos2(left + 62.0, cy), vec2(124.0, 124.0));
        match loc.kind {
            LocKind::Home => {
                icons::home(&p, icon.shrink(6.0), pal.file);
                p.rect_filled(
                    Rect::from_center_size(pos2(icon.center().x, icon.bottom() - 34.0), vec2(24.0, 36.0)),
                    CornerRadius::same(3),
                    Color32::from_rgb(226, 160, 96),
                );
            }
            LocKind::Volume => icons::drive(&p, icon, Color32::from_rgb(196, 208, 226), Color32::from_rgb(90, 108, 140)),
            _ => icons::folder(&p, icon, pal.folder, pal.folder_tab),
        }
        let tx = icon.right() + 26.0;
        let text_w = (block_w - 150.0).max(120.0);
        let name = widgets::fit_middle(ui, &loc.name, &semibold(23.0), text_w);
        p.text(pos2(tx, cy - 54.0), Align2::LEFT_CENTER, name, semibold(23.0), pal.text);
        let (created, modified) = if self.opts.demo {
            (1_735_300_000, 1_791_700_000)
        } else {
            (platform::created_secs(&loc.path), platform::modified_secs(&loc.path))
        };
        let location = loc.path.parent().map(platform::display_name).filter(|s| !s.is_empty()).unwrap_or_else(|| "—".into());
        let mut lines: Vec<(&str, String)> = vec![
            (t("Kind:"), if loc.kind == LocKind::Volume { t("Volume").to_string() } else { t("Folder").to_string() }),
            (t("Created:"), util::fmt_date(created, true)),
            (t("Modified:"), util::fmt_date(modified, true)),
        ];
        match self.space_of(&loc.path) {
            Some(space) if loc.kind == LocKind::Volume => {
                lines.push((t("Capacity:"), util::fmt_size(space.total)));
                lines.push((t("Available:"), util::fmt_size(space.free)));
            }
            _ => lines.push((t("Where:"), location)),
        }
        let label_w = lines.iter().map(|l| widgets::text_width(ui, l.0, &regular(14.5))).fold(0.0, f32::max);
        for (i, (label, value)) in lines.iter().enumerate() {
            let y = cy - 18.0 + i as f32 * 23.0;
            p.text(pos2(tx, y), Align2::LEFT_CENTER, *label, regular(14.5), pal.text_dim);
            let value = widgets::fit_middle(ui, value, &regular(14.5), (text_w - label_w - 14.0).max(60.0));
            p.text(pos2(tx + label_w + 14.0, y), Align2::LEFT_CENTER, value, regular(14.5), pal.text);
        }
        let hint = widgets::fit_end(ui, t("Click the Start Scanning button to review its content"), &regular(14.5), rect.width() - 40.0);
        p.text(pos2(left, cy + 108.0), Align2::LEFT_CENTER, hint, regular(14.5), pal.text_dim);
    }

    // ---- results -----------------------------------------------------------------

    fn content(&self, ui: &mut Ui, rect: Rect, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let flat = s.search_open || s.tab == Tab::Biggest;
        let chart_min = if self.settings.show_chart { 300.0 } else { 0.0 };
        let lists_right;
        if flat {
            let w = if self.settings.show_chart { FLAT_W.min(rect.width() - chart_min).max(220.0) } else { rect.width() };
            let list = Rect::from_min_size(rect.min, vec2(w, rect.height()));
            self.flat_list(ui, list, s, acts);
            lists_right = list.right();
        } else {
            let fit = (((rect.width() - chart_min) / COL_W).floor() as usize).clamp(1, s.columns.len().max(1));
            let skip = s.columns.len().saturating_sub(fit);
            let mut x = rect.left();
            for (ci, col) in s.columns.iter().enumerate().skip(skip) {
                let w = if self.settings.show_chart { COL_W.min(rect.width()) } else { COL_W.max(rect.width() / fit as f32) };
                let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
                self.column(ui, r, ci, col, s, acts);
                x += w;
            }
            lists_right = x;
        }
        ui.painter().line_segment(
            [pos2(lists_right, rect.top()), pos2(lists_right, rect.bottom())],
            Stroke::new(1.0_f32, pal.panel_line.gamma_multiply(0.6)),
        );
        if self.settings.show_chart {
            let area = Rect::from_min_max(pos2(lists_right + 1.0, rect.top()), rect.max);
            if area.width() > 120.0 {
                self.chart(ui, area, s, acts);
            }
        }
    }

    fn column(&self, ui: &mut Ui, rect: Rect, ci: usize, col: &Column, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        if ci > 0 {
            ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0_f32, pal.panel_line.gamma_multiply(0.6)));
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(0.0, 4.0))).id_salt(("column", ci)));
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if col.items.is_empty() {
            let text = if col.denied {
                t("This folder could not be read")
            } else if s.scan.is_done() {
                t("This folder is empty")
            } else {
                t("Scanning…")
            };
            child.painter().text(pos2(rect.center().x, rect.top() + 40.0), Align2::CENTER_CENTER, text, regular(13.0), pal.text_faint);
            return;
        }
        let selected = s.open.get(ci).copied();
        egui::ScrollArea::vertical()
            .id_salt(("column-scroll", ci, col.dir))
            .auto_shrink([false, false])
            .show_rows(&mut child, ROW_H, col.items.len(), |ui, range| {
                for item in &col.items[range] {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    match item {
                        Item::Row(row) => self.row(ui, r, resp, row, Some(ci), selected == Some(row.id), false, s, acts),
                        Item::Member(row) => self.row(ui, r, resp, row, Some(ci), selected == Some(row.id), true, s, acts),
                        Item::Group { kind, size, count, open } => {
                            if resp.hovered() {
                                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), CornerRadius::same(6), pal.row_hover);
                            }
                            let p = ui.painter();
                            icons::chevron(p, Rect::from_center_size(pos2(r.left() + 19.0, r.center().y), vec2(13.0, 13.0)), pal.accent_hover, *open);
                            icons::group_dot(p, Rect::from_center_size(pos2(r.left() + 46.0, r.center().y), vec2(20.0, 20.0)), pal.text_faint);
                            let label = if *kind == GroupKind::Small { t("Small items") } else { t("Hidden items") };
                            p.text(pos2(r.left() + 64.0, r.center().y), Align2::LEFT_CENTER, label, regular(13.0), pal.text_faint);
                            p.text(pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, util::fmt_size(*size), regular(13.0), pal.text_faint);
                            if resp.clicked() {
                                acts.push(Action::ToggleGroup(col.dir, *kind));
                            }
                            resp.on_hover_text(tf("{} items", &[&util::fmt_count(*count as u64)]));
                        }
                        Item::More(n) => {
                            let text = tf("{} more items", &[&util::fmt_count(*n as u64)]);
                            ui.painter().text(pos2(r.left() + 64.0, r.center().y), Align2::LEFT_CENTER, text, regular(12.5), pal.text_faint);
                        }
                        Item::Pseudo { kind, size } => {
                            let (label, color) = match kind {
                                SegKind::Free => (t("Free space"), pal.seg_free.gamma_multiply(3.0)),
                                _ => (t("Other used space"), pal.seg_other),
                            };
                            let p = ui.painter();
                            icons::group_dot(p, Rect::from_center_size(pos2(r.left() + 46.0, r.center().y), vec2(20.0, 20.0)), color);
                            p.text(pos2(r.left() + 64.0, r.center().y), Align2::LEFT_CENTER, label, regular(13.0), pal.text_dim);
                            p.text(pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, util::fmt_size(*size), regular(13.0), pal.text_dim);
                            if *kind == SegKind::Other {
                                resp.on_hover_text(t("Space used on this volume that the scan could not attribute: system files, snapshots, or folders that could not be read."));
                            }
                        }
                    }
                }
            });
    }

    /// One file or folder line of an outline column.
    #[allow(clippy::too_many_arguments)]
    fn row(&self, ui: &mut Ui, r: Rect, resp: egui::Response, row: &Row, col: Option<usize>, selected: bool, member: bool, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let hovered = resp.hovered() || s.hover == Some(row.id);
        let bg = r.shrink2(vec2(4.0, 1.0));
        if selected {
            ui.painter().rect_filled(bg, CornerRadius::same(6), pal.row_selected);
        } else if hovered {
            ui.painter().rect_filled(bg, CornerRadius::same(6), pal.row_hover);
        }
        let dim = row.hidden;
        let indent = if member { 4.0 } else { 0.0 };
        let cb = Rect::from_center_size(pos2(r.left() + 19.0 + indent, r.center().y), vec2(16.0, 16.0));
        let can_check = !row.hidden || self.settings.allow_removing_hidden;
        widgets::checkbox(ui.painter(), cb, row.check, pal, can_check);
        let cb_resp = ui.interact(cb.expand(6.0), resp.id.with("check"), Sense::click());
        if cb_resp.clicked() {
            acts.push(Action::ToggleCheck(row.id));
        }
        let icon = Rect::from_center_size(pos2(r.left() + 46.0 + indent, r.center().y), vec2(20.0, 20.0));
        widgets::item_icon(ui.painter(), icon, row.flags, pal, dim);

        let size = util::fmt_size(row.size);
        let size_color = if dim { pal.text_faint } else { pal.size[util::size_class(row.size) as usize] };
        let size_w = widgets::text_width(ui, &size, &regular(13.5));
        ui.painter().text(pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, size, regular(13.5), size_color);
        let name_x = r.left() + 64.0 + indent;
        let name = widgets::fit_middle(ui, &row.name, &regular(13.5), r.right() - 14.0 - size_w - 8.0 - name_x);
        let truncated = name != row.name;
        let ink = if dim { pal.text_faint } else { pal.text };
        ui.painter().text(pos2(name_x, r.center().y), Align2::LEFT_CENTER, name, regular(13.5), ink);

        if resp.hovered() || cb_resp.hovered() {
            acts.push(Action::Hover(row.id, false));
        }
        if resp.double_clicked() && row.flags & F_DIR != 0 {
            acts.push(Action::Drill(row.id));
        } else if resp.clicked() {
            match col {
                Some(c) => acts.push(Action::RowClick { col: c, id: row.id }),
                None => acts.push(Action::FlatClick(row.id)),
            }
        }
        resp.context_menu(|ui| self.item_menu(ui, row.id, row.flags, row.check, acts));
        if truncated {
            resp.on_hover_text(&row.name);
        }
    }

    fn item_menu(&self, ui: &mut Ui, id: NodeId, flags: u8, check: Check, acts: &mut Vec<Action>) {
        let mut entry = |ui: &mut Ui, label: &str, action: Action| {
            if ui.button(label).clicked() {
                acts.push(action);
                ui.close_menu();
            }
        };
        entry(ui, reveal_label(), Action::On(Cmd::Reveal, id));
        entry(ui, t("Get Information"), Action::On(Cmd::Info, id));
        entry(ui, t(platform::preview_label()), Action::On(Cmd::QuickLook, id));
        ui.separator();
        entry(ui, t("Copy Path"), Action::On(Cmd::CopyPath, id));
        ui.separator();
        entry(ui, if check == Check::Yes { t("Deselect") } else { t("Select") }, Action::ToggleCheck(id));
        ui.separator();
        entry(ui, t("Quick Remove"), Action::QuickRemove(id));
        entry(ui, t("Move to Folder…"), Action::On(Cmd::Move, id));
        entry(ui, t("Copy to Folder…"), Action::On(Cmd::Copy, id));
        if flags & F_DIR != 0 {
            ui.separator();
            entry(ui, t("Go to Folder"), Action::Drill(id));
            entry(ui, t("Add Folder to Favorites"), Action::On(Cmd::AddFavorite, id));
        }
    }

    /// Biggest files or search results: two-line rows with the parent path.
    fn flat_list(&self, ui: &mut Ui, rect: Rect, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let searching = s.search_open;
        let rows: &[Row] = if searching { &s.search_rows } else { &s.biggest };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(0.0, 4.0))).id_salt("flat-list"));
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if rows.is_empty() {
            let text = if !searching {
                if s.scan.is_done() { t("No files found") } else { t("Scanning…") }
            } else if s.query.trim().is_empty() {
                t("Type a name to search the scanned items")
            } else if s.searching {
                t("Searching…")
            } else {
                t("No items match your search")
            };
            let text = widgets::fit_end(ui, text, &regular(13.0), rect.width() - 20.0);
            child.painter().text(pos2(rect.center().x, rect.top() + 44.0), Align2::CENTER_CENTER, text, regular(13.0), pal.text_faint);
            return;
        }
        let more = !searching && s.biggest_more;
        egui::ScrollArea::vertical().id_salt(("flat-scroll", searching)).auto_shrink([false, false]).show_rows(
            &mut child,
            FLAT_ROW_H,
            rows.len() + more as usize,
            |ui, range| {
                for i in range {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), FLAT_ROW_H), Sense::click());
                    let Some(row) = rows.get(i) else {
                        let b = Rect::from_center_size(r.center(), vec2((r.width() - 60.0).min(240.0), 32.0));
                        if widgets::button(ui, b, t("View more biggest files"), Btn::Outline, true, pal, regular(13.0)).clicked() {
                            acts.push(Action::MoreBiggest);
                        }
                        continue;
                    };
                    self.flat_row(ui, r, resp, row, s, acts);
                }
            },
        );
    }

    fn flat_row(&self, ui: &mut Ui, r: Rect, resp: egui::Response, row: &Row, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let bg = r.shrink2(vec2(4.0, 1.0));
        if s.focus == Some(row.id) {
            ui.painter().rect_filled(bg, CornerRadius::same(6), pal.row_selected);
        } else if resp.hovered() || s.hover == Some(row.id) {
            ui.painter().rect_filled(bg, CornerRadius::same(6), pal.row_hover);
        }
        let dim = row.hidden;
        let cb = Rect::from_center_size(pos2(r.left() + 19.0, r.center().y), vec2(16.0, 16.0));
        widgets::checkbox(ui.painter(), cb, row.check, pal, !row.hidden || self.settings.allow_removing_hidden);
        let cb_resp = ui.interact(cb.expand(6.0), resp.id.with("check"), Sense::click());
        if cb_resp.clicked() {
            acts.push(Action::ToggleCheck(row.id));
        }
        let icon = Rect::from_center_size(pos2(r.left() + 50.0, r.center().y), vec2(26.0, 26.0));
        widgets::item_icon(ui.painter(), icon, row.flags, pal, dim);
        let size = util::fmt_size(row.size);
        let size_w = widgets::text_width(ui, &size, &regular(13.5));
        let size_color = if dim { pal.text_faint } else { pal.size[util::size_class(row.size) as usize] };
        ui.painter().text(pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, size, regular(13.5), size_color);
        let x = r.left() + 74.0;
        let text_w = r.right() - 14.0 - size_w - 10.0 - x;
        let name = widgets::fit_middle(ui, &row.name, &regular(13.5), text_w);
        ui.painter().text(pos2(x, r.center().y - 9.0), Align2::LEFT_CENTER, name, regular(13.5), if dim { pal.text_dim } else { pal.text });
        let path = widgets::fit_end(ui, &row.crumbs.join(" › "), &regular(11.5), text_w);
        ui.painter().text(pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, path, regular(11.5), pal.text_faint);

        if resp.hovered() || cb_resp.hovered() {
            acts.push(Action::Hover(row.id, false));
        }
        if resp.double_clicked() {
            acts.push(Action::SelectPath(row.id));
        } else if resp.clicked() {
            acts.push(Action::FlatClick(row.id));
        }
        resp.context_menu(|ui| {
            self.item_menu(ui, row.id, row.flags, row.check, acts);
            ui.separator();
            if ui.button(t("Show in Folders Outline")).clicked() {
                acts.push(Action::SelectPath(row.id));
                ui.close_menu();
            }
        });
        resp.on_hover_text(format!("{}\n{}", row.name, row.crumbs.join(" › ")));
    }

    // ---- chart -------------------------------------------------------------------

    fn chart(&self, ui: &mut Ui, rect: Rect, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let l = &s.layout;
        let g = Geo::fit(rect.shrink(16.0), l.rings);
        let resp = ui.interact(rect, ui.id().with("chart"), Sense::click());
        let p = ui.painter_at(rect);

        let mut mesh = egui::Mesh::default();
        for (i, seg) in l.segs.iter().enumerate() {
            let checked = s.seg_checked.get(i).copied().unwrap_or(false);
            chart::add_segment(&mut mesh, &g, seg, chart::seg_color(l, seg, i, pal, checked));
        }
        p.add(Shape::mesh(mesh));

        let hit = resp.hover_pos().map(|pos| chart::hit(l, &g, pos)).unwrap_or(Hit::None);
        let find = |id: NodeId| l.segs.iter().position(|x| x.kind == SegKind::Node && x.id == id);

        // The item selected in the lists keeps a thin outline.
        let marked = if s.search_open || s.tab == Tab::Biggest { s.focus } else { s.open.last().copied() };
        if let Some(i) = marked.and_then(find) {
            p.add(Shape::closed_line(chart::outline(&g, &l.segs[i]), Stroke::new(1.5_f32, Color32::from_white_alpha(200))));
        }

        let mut highlight = match hit {
            Hit::Seg(i) => Some(i),
            _ => None,
        };
        if highlight.is_none() && hit != Hit::Center {
            highlight = s.hover.and_then(find);
        }
        if let Some(i) = highlight {
            let seg = &l.segs[i];
            p.add(Shape::closed_line(chart::outline(&g, seg), Stroke::new(2.5_f32, Color32::WHITE)));
            let (title, lines) = self.segment_info(s, seg);
            widgets::info_popover(ui.ctx(), g.anchor(seg), ui.ctx().screen_rect(), &title, &lines, pal);
        }

        let scanning = !s.scan.is_done();
        chart::paint_center(&p, &g, pal, hit == Hit::Center && s.view_root != 0);
        if scanning {
            widgets::spinner(&p, g.center, g.r0 + 5.0, pal.center_ring, ui.input(|i| i.time * 0.4));
        }
        let font = semibold((g.r0 * 0.2).clamp(12.0, 17.0));
        let label = widgets::fit_middle(ui, &s.view_name, &font, g.r0 * 1.7);
        p.text(g.center, Align2::CENTER_CENTER, label, font, Color32::WHITE);
        if l.segs.is_empty() && !scanning {
            p.text(g.center + vec2(0.0, g.r0 + 26.0), Align2::CENTER_CENTER, t("This folder is empty"), regular(13.0), pal.text_faint);
        }

        match hit {
            Hit::Center => {
                if s.view_root != 0 {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        acts.push(Action::Cmd(Cmd::GoParent));
                    }
                }
                let (title, lines) = self.root_info(s);
                let anchor = g.center + vec2(g.r0 * 0.72, -g.r0 * 0.72);
                widgets::info_popover(ui.ctx(), anchor, ui.ctx().screen_rect(), &title, &lines, pal);
            }
            Hit::Seg(i) => {
                let seg = l.segs[i];
                match seg.kind {
                    SegKind::Node => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        acts.push(Action::Hover(seg.id, true));
                        if resp.double_clicked() {
                            acts.push(Action::Drill(seg.id));
                        } else if resp.clicked() {
                            acts.push(Action::SelectPath(seg.id));
                        }
                    }
                    SegKind::Small => {
                        if resp.clicked() && seg.id != s.view_root {
                            acts.push(Action::SelectPath(seg.id));
                        }
                    }
                    _ => {}
                }
            }
            Hit::None => {}
        }
    }

    fn segment_info(&self, s: &Session, seg: &chart::Seg) -> (String, Vec<(String, String)>) {
        let size = (t("Size:").to_string(), util::fmt_size(seg.size));
        match seg.kind {
            SegKind::Node => {
                let tree = s.scan.tree.read();
                let n = tree.node(seg.id);
                let mut lines = vec![(t("Size:").to_string(), util::fmt_size(n.size))];
                if n.is_dir() {
                    lines.push((t("Files:").to_string(), util::fmt_count(n.items as u64)));
                }
                lines.push((t("Modified:").to_string(), util::fmt_date(n.mtime, false)));
                (n.display_name(), lines)
            }
            SegKind::Small => (t("Small items").to_string(), vec![size]),
            SegKind::Free => (t("Free space").to_string(), vec![size]),
            SegKind::Other => (t("Other used space").to_string(), vec![size]),
        }
    }

    fn root_info(&self, s: &Session) -> (String, Vec<(String, String)>) {
        let tree = s.scan.tree.read();
        let n = tree.node(s.view_root);
        (
            s.view_name.clone(),
            vec![
                (t("Size:").to_string(), util::fmt_size(n.size)),
                (t("Files:").to_string(), util::fmt_count(n.items as u64)),
                (t("Modified:").to_string(), util::fmt_date(n.mtime, false)),
            ],
        )
    }

    // ---- breadcrumbs ---------------------------------------------------------------

    fn crumbs(&self, ui: &mut Ui, rect: Rect, s: &Session, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        ui.painter().rect_filled(rect, CornerRadius::ZERO, pal.bar);
        let font = regular(12.5);
        let widths: Vec<f32> = s.crumbs.iter().map(|c| widgets::text_width(ui, &c.1, &font).min(220.0) + 22.0).collect();
        // Drop leading folders when the path does not fit.
        let mut first = 0;
        let mut total: f32 = widths.iter().sum::<f32>() + 16.0 * widths.len() as f32;
        while total > rect.width() - 40.0 && first + 1 < widths.len() {
            total -= widths[first] + 16.0;
            first += 1;
        }
        let mut x = rect.left() + 14.0;
        let cy = rect.center().y;
        if first > 0 {
            ui.painter().text(pos2(x, cy), Align2::LEFT_CENTER, "…  ›", font.clone(), pal.text_faint);
            x += 30.0;
        }
        for (i, (id, name)) in s.crumbs.iter().enumerate().skip(first) {
            let w = widths[i];
            let r = Rect::from_min_size(pos2(x - 3.0, rect.top() + 3.0), vec2(w + 4.0, rect.height() - 6.0));
            let resp = ui.interact(r, ui.id().with(("crumb", i)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(5), pal.row_hover);
            }
            icons::folder(ui.painter(), Rect::from_center_size(pos2(x + 7.0, cy), vec2(14.0, 14.0)), pal.folder, pal.folder_tab);
            let label = widgets::fit_middle(ui, name, &font, 220.0);
            ui.painter().text(pos2(x + 19.0, cy), Align2::LEFT_CENTER, label, font.clone(), pal.text_dim);
            if resp.clicked() {
                acts.push(Action::Crumb(*id));
            }
            x += w + 4.0;
            if i + 1 < s.crumbs.len() {
                ui.painter().text(pos2(x + 2.0, cy), Align2::LEFT_CENTER, "›", font.clone(), pal.text_faint);
                x += 12.0;
            }
        }
    }

    // ---- footer --------------------------------------------------------------------

    fn footer(&self, ui: &mut Ui, rect: Rect, session: Option<&Session>, acts: &mut Vec<Action>) {
        let pal = &self.pal;
        let cy = rect.center().y;
        let button = Rect::from_min_max(pos2(rect.right() - 16.0 - 250.0f32.min(rect.width() * 0.45), cy - 23.0), pos2(rect.right() - 16.0, cy + 23.0));
        let left = rect.left() + 20.0;
        let text_room = button.left() - left - 16.0;
        let Some(s) = session else {
            let enabled = self.current.is_some();
            if widgets::button(ui, button, t("Start Scanning"), Btn::Primary, enabled, pal, semibold(15.5)).clicked() {
                acts.push(Action::Cmd(Cmd::Scan));
            }
            let hint = widgets::fit_end(ui, t("Drag and drop a disk or folder here to scan it"), &regular(13.0), text_room);
            ui.painter().text(pos2(left, cy), Align2::LEFT_CENTER, hint, regular(13.0), pal.text_faint);
            return;
        };

        if !s.scan.is_done() {
            if widgets::button(ui, button, t("Stop Scanning"), Btn::Primary, true, pal, semibold(15.5)).clicked() {
                acts.push(Action::Cmd(Cmd::Stop));
            }
            let n = s.scan.entries.load(std::sync::atomic::Ordering::Relaxed);
            let text = tf("Scanning… {} items found", &[&util::fmt_count(n)]);
            let text = widgets::fit_end(ui, &text, &regular(13.0), text_room);
            ui.painter().text(pos2(left, cy), Align2::LEFT_CENTER, text, regular(13.0), pal.text_dim);
            return;
        }

        // Left: what is selected, or a summary of the scan.
        if s.checked.is_empty() {
            let secs = s.scan.elapsed().as_secs_f32();
            let mut text = tn("{} files", s.total_items as u64, &[&util::fmt_count(s.total_items as u64)]);
            if secs > 0.05 {
                text = format!("{text}  ·  {}", tf("scanned in {} s", &[&format!("{secs:.1}")]));
            }
            if s.scan.was_cancelled() {
                text = format!("{text}  ·  {}", t("scan stopped, results are partial"));
            }
            let warn = s.denied > 0;
            let y = if warn { cy - 10.0 } else { cy };
            let text = widgets::fit_end(ui, &text, &regular(13.0), text_room);
            ui.painter().text(pos2(left, y), Align2::LEFT_CENTER, text, regular(13.0), pal.text_dim);
            if warn {
                let msg = tn("{} folders could not be read", s.denied as u64, &[&util::fmt_count(s.denied as u64)]);
                let msg = widgets::fit_end(ui, &msg, &regular(12.5), text_room - 24.0);
                let w = widgets::text_width(ui, &msg, &regular(12.5));
                let r = Rect::from_min_size(pos2(left - 2.0, cy + 2.0), vec2(w + 28.0, 20.0));
                let resp = ui.interact(r, ui.id().with("denied"), Sense::click());
                icons::warning(ui.painter(), Rect::from_center_size(pos2(left + 8.0, r.center().y), vec2(16.0, 16.0)), pal.size[1], pal.bar);
                let ink = if resp.hovered() { pal.text } else { pal.size[1] };
                ui.painter().text(pos2(left + 22.0, r.center().y), Align2::LEFT_CENTER, msg, regular(12.5), ink);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    acts.push(Action::ShowDenied);
                }
            }
        } else {
            let (num, unit) = util::size_parts(s.checked_size);
            let mut x = left;
            let big = semibold(28.0);
            let small = regular(11.5);
            let stat = |ui: &mut Ui, x: &mut f32, number: &str, top: &str, bottom: &str| {
                let w = widgets::text_width(ui, number, &big);
                ui.painter().text(pos2(*x, cy), Align2::LEFT_CENTER, number, big.clone(), pal.text);
                ui.painter().text(pos2(*x + w + 8.0, cy - 7.0), Align2::LEFT_CENTER, top, small.clone(), pal.text);
                ui.painter().text(pos2(*x + w + 8.0, cy + 8.0), Align2::LEFT_CENTER, bottom, small.clone(), pal.text_dim);
                let label_w = widgets::text_width(ui, top, &small).max(widgets::text_width(ui, bottom, &small));
                *x += w + 8.0 + label_w + 14.0;
            };
            stat(ui, &mut x, &num, unit, t("selected"));
            if x + 110.0 < button.left() {
                ui.painter().line_segment([pos2(x - 4.0, cy - 18.0), pos2(x - 4.0, cy + 18.0)], Stroke::new(1.0_f32, pal.panel_line));
                x += 10.0;
                let count = s.checked.len();
                stat(ui, &mut x, &util::fmt_count(count as u64), if count == 1 { t("Item") } else { t("Items") }, t("selected"));
            }
        }

        // Right: review button with a menu to pick the operation.
        let enabled = !s.checked.is_empty();
        let label = match self.op_mode {
            OpKind::Remove => t("Review and Remove"),
            OpKind::Move => t("Review and Move"),
            OpKind::Copy => t("Review and Copy"),
        };
        let rad = CornerRadius::same(12);
        let main = Rect::from_min_max(button.min, pos2(button.right() - 40.0, button.bottom()));
        let arrow = Rect::from_min_max(pos2(button.right() - 40.0, button.top()), button.max);
        let main_resp = ui.interact(main, ui.id().with("review"), if enabled { Sense::click() } else { Sense::hover() });
        let arrow_resp = ui.interact(arrow, ui.id().with("review-mode"), Sense::click());
        let fill = if !enabled {
            pal.control.gamma_multiply(0.85)
        } else if main_resp.hovered() {
            pal.accent_hover
        } else {
            pal.accent
        };
        let ink = if enabled { Color32::WHITE } else { pal.text_faint };
        ui.painter().rect_filled(button, rad, fill);
        if !enabled {
            ui.painter().rect_stroke(button, rad, Stroke::new(1.0_f32, pal.control_line.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        }
        if arrow_resp.hovered() {
            ui.painter().rect_filled(arrow.shrink(5.0), CornerRadius::same(8), Color32::from_white_alpha(26));
        }
        let text = widgets::fit_end(ui, label, &semibold(15.5), main.width() - 14.0);
        ui.painter().text(main.center() + vec2(8.0, 0.0), Align2::CENTER_CENTER, text, semibold(15.5), ink);
        icons::chevron(ui.painter(), Rect::from_center_size(arrow.center(), vec2(15.0, 15.0)), ink, true);
        if main_resp.clicked() {
            acts.push(Action::Review);
        }
        let popup = ui.id().with("review-popup");
        if arrow_resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup::popup_above_or_below_widget(
            ui,
            popup,
            &arrow_resp,
            egui::AboveOrBelow::Above,
            egui::PopupCloseBehavior::CloseOnClick,
            |ui| {
                ui.set_min_width(190.0);
                for (kind, text) in [(OpKind::Remove, t("Remove")), (OpKind::Move, t("Move to Folder…")), (OpKind::Copy, t("Copy to Folder…"))] {
                    if ui.selectable_label(self.op_mode == kind, text).clicked() {
                        acts.push(Action::SetOpMode(kind));
                    }
                }
            },
        );

        // First-use hint above the button.
        if enabled && self.settings.show_selection_hint && !self.hint_dismissed && matches!(self.dialog, crate::app::Dialog::None) {
            let text = t("Review the selected items, then confirm to move them to the Trash.");
            let font = regular(12.5);
            let galley = ui.fonts(|f| f.layout(text.to_owned(), font, pal.text, 250.0));
            let size = galley.size() + vec2(44.0, 20.0);
            let r = Rect::from_min_size(pos2(button.right() - size.x, rect.top() - CRUMB_H - size.y - 8.0), size);
            let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("selection-hint"));
            let hp = ui.ctx().layer_painter(layer);
            hp.rect_filled(r, CornerRadius::same(9), pal.popup);
            hp.rect_stroke(r, CornerRadius::same(9), Stroke::new(1.0_f32, pal.popup_line), egui::StrokeKind::Inside);
            hp.galley(r.min + vec2(12.0, 10.0), galley, pal.text);
            let close = Rect::from_center_size(pos2(r.right() - 16.0, r.top() + 16.0), vec2(20.0, 20.0));
            let resp = ui.interact(close, ui.id().with("hint-close"), Sense::click());
            hp.text(close.center(), Align2::CENTER_CENTER, "×", regular(16.0), if resp.hovered() { pal.text } else { pal.text_dim });
            if resp.clicked() {
                acts.push(Action::DismissHint);
            }
        }
    }

    fn paint_toast(&self, ui: &mut Ui, main: Rect) {
        let Some((text, _)) = &self.toast else { return };
        let pal = &self.pal;
        let font = regular(13.0);
        let galley = ui.fonts(|f| f.layout(text.clone(), font, pal.text, (main.width() - 80.0).min(520.0)));
        let size = galley.size() + vec2(32.0, 20.0);
        let r = Rect::from_center_size(pos2(main.center().x, main.bottom() - FOOTER_H - CRUMB_H - size.y * 0.5 - 14.0), size);
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toast"));
        let p = ui.ctx().layer_painter(layer);
        p.rect_filled(r.translate(vec2(0.0, 3.0)), CornerRadius::same(11), Color32::from_black_alpha(50));
        p.rect_filled(r, CornerRadius::same(10), pal.popup);
        p.rect_stroke(r, CornerRadius::same(10), Stroke::new(1.0_f32, pal.popup_line), egui::StrokeKind::Inside);
        p.galley(r.min + vec2(16.0, 10.0), galley, pal.text);
    }
}

pub fn reveal_label() -> &'static str {
    match platform::file_manager_name() {
        "Finder" => t("Show in Finder"),
        "Explorer" => t("Show in Explorer"),
        _ => t("Show in File Manager"),
    }
}
