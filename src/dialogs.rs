//! Modal dialogs: review, progress, summary, information, settings, help.

use crate::app::{App, Dialog};
use crate::i18n::{self, t, tf, tn};
use crate::ops::OpKind;
use crate::settings::{Appearance, Settings};
use crate::theme::{regular, semibold};
use crate::{icons, platform, util, widgets};
use egui::{vec2, Color32, RichText, Sense, Ui};
use std::sync::atomic::Ordering;

enum Outcome {
    Keep,
    Close,
    Confirm,
    Reveal(std::path::PathBuf),
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(17.0)));
    ui.add_space(6.0);
}

fn primary(ui: &mut Ui, text: &str, fill: Color32) -> bool {
    let b = egui::Button::new(RichText::new(text).font(semibold(13.5)).color(Color32::WHITE)).fill(fill).min_size(vec2(110.0, 32.0));
    ui.add(b).clicked()
}

fn secondary(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).font(regular(13.5))).min_size(vec2(96.0, 32.0))).clicked()
}

/// Row of right-aligned buttons at the bottom of a dialog.
fn buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
}

impl App {
    pub fn draw_dialog(&mut self, ctx: &egui::Context) {
        if matches!(self.dialog, Dialog::None) {
            return;
        }
        let pal = self.pal;
        let demo = self.opts.demo;
        let frame = egui::Frame::popup(&ctx.style())
            .fill(pal.popup)
            .stroke(egui::Stroke::new(1.0_f32, pal.popup_line))
            .corner_radius(egui::CornerRadius::same(14))
            .inner_margin(egui::Margin::same(22));
        let modal = egui::Modal::new(egui::Id::new("dialog")).frame(frame).backdrop_color(Color32::from_black_alpha(120));
        let mut outcome = Outcome::Keep;
        let mut settings_changed = false;
        let response = modal.show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 7.0);
            match &mut self.dialog {
                Dialog::None => {}
                Dialog::Review { kind, items, dest, .. } => {
                    ui.set_width(540.0);
                    let (heading, lead, action) = match kind {
                        OpKind::Remove => (t("Review and confirm removal"), t("These items will be moved to the Trash:"), t("Remove")),
                        OpKind::Move => (t("Review and confirm moving"), t("These items will be moved to:"), t("Move")),
                        OpKind::Copy => (t("Review and confirm copying"), t("These items will be copied to:"), t("Copy")),
                    };
                    title(ui, heading);
                    ui.label(RichText::new(lead).color(pal.text_dim));
                    if let Some(d) = dest {
                        ui.label(RichText::new(d.to_string_lossy()).font(semibold(13.0)));
                    }
                    ui.add_space(4.0);
                    let row_h = 42.0;
                    egui::Frame::NONE
                        .fill(pal.bar)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                            egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show_rows(ui, row_h, items.len(), |ui, range| {
                                for it in &items[range] {
                                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                                    let icon = egui::Rect::from_center_size(egui::pos2(r.left() + 22.0, r.center().y), vec2(24.0, 24.0));
                                    let flags = if it.item.is_dir { crate::tree::F_DIR } else { 0 };
                                    widgets::item_icon(ui.painter(), icon, flags, &pal, false);
                                    let size = util::fmt_size(it.item.size);
                                    let size_w = widgets::text_width(ui, &size, &regular(13.0));
                                    ui.painter().text(
                                        egui::pos2(r.right() - 12.0, r.center().y),
                                        egui::Align2::RIGHT_CENTER,
                                        size,
                                        regular(13.0),
                                        pal.size[util::size_class(it.item.size) as usize],
                                    );
                                    let x = r.left() + 44.0;
                                    let w = r.right() - 12.0 - size_w - 12.0 - x;
                                    let name = widgets::fit_middle(ui, &it.name, &regular(13.5), w);
                                    ui.painter().text(egui::pos2(x, r.center().y - 8.0), egui::Align2::LEFT_CENTER, name, regular(13.5), pal.text);
                                    let sub = if it.hidden { format!("{}  ·  {}", t("Hidden item"), it.parent) } else { it.parent.clone() };
                                    let sub = widgets::fit_middle(ui, &sub, &regular(11.5), w);
                                    let ink = if it.hidden { pal.size[1] } else { pal.text_faint };
                                    ui.painter().text(egui::pos2(x, r.center().y + 9.0), egui::Align2::LEFT_CENTER, sub, regular(11.5), ink);
                                }
                            });
                        });
                    ui.add_space(4.0);
                    let total: u64 = items.iter().map(|i| i.item.size).sum();
                    let count = util::fmt_count(items.len() as u64);
                    ui.label(RichText::new(format!("{}  ·  {}", tn("{} items", items.len() as u64, &[&count]), util::fmt_size(total))).font(semibold(13.5)));
                    if *kind != OpKind::Copy && items.iter().any(|i| i.hidden) {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                            icons::warning(ui.painter(), r, pal.size[1], pal.popup);
                            ui.label(RichText::new(t("Removing hidden items is not recommended.")).color(pal.size[1]));
                        });
                    }
                    let fill = if *kind == OpKind::Remove { pal.danger } else { pal.accent };
                    buttons(ui, |ui| {
                        if primary(ui, action, fill) {
                            outcome = Outcome::Confirm;
                        }
                        if secondary(ui, t("Cancel")) {
                            outcome = Outcome::Close;
                        }
                    });
                }
                Dialog::Progress { progress, .. } => {
                    ui.set_width(400.0);
                    title(
                        ui,
                        match progress.kind {
                            OpKind::Remove => t("Removing selected items…"),
                            OpKind::Move => t("Moving selected items…"),
                            OpKind::Copy => t("Copying selected items…"),
                        },
                    );
                    let done = progress.done.load(Ordering::Acquire);
                    let fraction = if progress.total == 0 { 1.0 } else { done as f32 / progress.total as f32 };
                    ui.add(egui::ProgressBar::new(fraction).desired_width(400.0).fill(pal.accent));
                    ui.label(RichText::new(format!("{done} / {}", progress.total)).color(pal.text_dim));
                    ui.label(RichText::new(t("The process can take some time. Please wait until it is finished.")).color(pal.text_dim));
                    buttons(ui, |ui| {
                        if secondary(ui, t("Stop")) {
                            progress.cancel.store(true, Ordering::Release);
                        }
                    });
                }
                Dialog::Summary(sum) => {
                    ui.set_width(460.0);
                    let clean = sum.failed.is_empty();
                    title(
                        ui,
                        match (sum.kind, clean) {
                            (OpKind::Remove, true) => t("Removal successfully completed"),
                            (OpKind::Remove, false) => t("Removal completed, some items were not removed"),
                            (OpKind::Move, true) => t("Moving successfully completed"),
                            (OpKind::Move, false) => t("Moving completed, some items were not moved"),
                            (OpKind::Copy, true) => t("Copying successfully completed"),
                            (OpKind::Copy, false) => t("Copying completed, some items were not copied"),
                        },
                    );
                    let n = util::fmt_count(sum.ok as u64);
                    let size = util::fmt_size(sum.size);
                    ui.label(match sum.kind {
                        OpKind::Remove => tn("{} items moved to the Trash ({}).", sum.ok as u64, &[&n, &size]),
                        OpKind::Move => tn("{} items moved ({}).", sum.ok as u64, &[&n, &size]),
                        OpKind::Copy => tn("{} items copied ({}).", sum.ok as u64, &[&n, &size]),
                    });
                    if sum.kind == OpKind::Remove && sum.ok > 0 {
                        ui.label(RichText::new(t("Empty the Trash to free up the space.")).color(pal.text_dim));
                    }
                    if !sum.renamed.is_empty() || !clean {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for (from, to) in &sum.renamed {
                            ui.label(RichText::new(tf("“{}” was renamed to “{}” because the name was taken.", &[from, to])).color(pal.text_dim));
                        }
                        if !clean {
                            ui.add_space(4.0);
                            ui.label(RichText::new(t("Items that could not be processed:")).font(semibold(13.0)));
                            for (name, error) in &sum.failed {
                                ui.label(RichText::new(format!("{name} — {error}")).color(pal.size[2]));
                            }
                        }
                        });
                    }
                    buttons(ui, |ui| {
                        if primary(ui, t("OK"), pal.accent) {
                            outcome = Outcome::Close;
                        }
                    });
                }
                Dialog::Info(info) => {
                    ui.set_width(440.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                        widgets::item_icon(ui.painter(), r, info.flags, &pal, false);
                        ui.label(RichText::new(util::middle_ellipsis(&info.name, 44)).font(semibold(17.0)));
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("info-grid").num_columns(2).spacing(vec2(16.0, 7.0)).show(ui, |ui| {
                        let line = |ui: &mut Ui, label: &str, value: String| {
                            ui.label(RichText::new(label).color(pal.text_dim));
                            ui.add(egui::Label::new(value).wrap());
                            ui.end_row();
                        };
                        line(ui, t("Kind:"), t(info.kind).to_string());
                        if let Some(size) = info.size {
                            line(ui, t("Size:"), format!("{} ({} {})", util::fmt_size(size), util::fmt_count(size), t("bytes")));
                        }
                        if let Some(items) = info.items {
                            line(ui, t("Files:"), util::fmt_count(items as u64));
                        }
                        line(ui, t("Created:"), util::fmt_date(info.created, true));
                        line(ui, t("Modified:"), util::fmt_date(info.modified, true));
                        line(ui, t("Where:"), info.path.to_string_lossy().into_owned());
                    });
                    let path = info.path.clone();
                    buttons(ui, |ui| {
                        if primary(ui, t("Close"), pal.accent) {
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, crate::views::reveal_label()) {
                            outcome = Outcome::Reveal(path);
                        }
                    });
                }
                Dialog::Settings => {
                    ui.set_width(430.0);
                    title(ui, t("Settings"));
                    let s = &mut self.settings;
                    egui::Grid::new("settings-grid").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| {
                        ui.label(t("Appearance mode:"));
                        let before = s.appearance;
                        let name = |a: Appearance| match a {
                            Appearance::Auto => t("Auto"),
                            Appearance::Light => t("Light"),
                            Appearance::Dark => t("Dark"),
                        };
                        egui::ComboBox::from_id_salt("appearance").width(170.0).selected_text(name(s.appearance)).show_ui(ui, |ui| {
                            for a in [Appearance::Auto, Appearance::Light, Appearance::Dark] {
                                ui.selectable_value(&mut s.appearance, a, name(a));
                            }
                        });
                        settings_changed |= before != s.appearance;
                        ui.end_row();

                        ui.label(t("Language:"));
                        let current = i18n::LANGUAGES.iter().find(|l| l.0 == s.language).map(|l| l.1).unwrap_or_else(|| t("Auto"));
                        egui::ComboBox::from_id_salt("language").width(170.0).selected_text(current).show_ui(ui, |ui| {
                            if ui.selectable_label(s.language == "auto", t("Auto")).clicked() {
                                s.language = "auto".into();
                                settings_changed = true;
                            }
                            for (code, native) in i18n::LANGUAGES {
                                if ui.selectable_label(s.language == *code, *native).clicked() {
                                    s.language = (*code).into();
                                    settings_changed = true;
                                }
                            }
                        });
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    settings_changed |= ui.checkbox(&mut s.show_selection_hint, t("Show hint for selected items")).changed();
                    settings_changed |= ui.checkbox(&mut s.allow_removing_hidden, t("Allow removing hidden files and folders")).changed();
                    ui.label(RichText::new(t("Removing hidden items is not recommended.")).color(pal.text_dim).font(regular(12.0)));
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(RichText::new(t("Disk Access")).font(semibold(13.5)));
                    if cfg!(target_os = "macos") {
                        ui.label(
                            RichText::new(t("macOS protects some folders. Grant Full Disk Access to this app so that every folder can be measured."))
                                .color(pal.text_dim),
                        );
                        if secondary(ui, t("Open System Settings")) && !demo {
                            platform::open_disk_access_settings();
                        }
                    } else {
                        ui.label(RichText::new(t("Folders that your account cannot read are skipped and reported after the scan.")).color(pal.text_dim));
                    }
                    buttons(ui, |ui| {
                        if primary(ui, t("Close"), pal.accent) {
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, t("Reset Settings")) {
                            let favorites = std::mem::take(&mut s.favorites);
                            *s = Settings { favorites, ephemeral: s.ephemeral, ..Settings::default() };
                            settings_changed = true;
                        }
                    });
                }
                Dialog::About => {
                    ui.set_width(400.0);
                    title(ui, t("Sunburst Disk Analyser"));
                    ui.label(RichText::new(format!("{} {}", t("Version"), env!("CARGO_PKG_VERSION"))).color(pal.text_dim));
                    ui.add_space(4.0);
                    ui.label(t("See what takes up space on your disks with an interactive sunburst chart, then remove, move or copy what you no longer need."));
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("Free and open-source software, MIT license.")).color(pal.text_dim));
                    ui.hyperlink(env!("CARGO_PKG_REPOSITORY"));
                    buttons(ui, |ui| {
                        if primary(ui, t("Close"), pal.accent) {
                            outcome = Outcome::Close;
                        }
                    });
                }
                Dialog::Help => {
                    ui.set_width(500.0);
                    title(ui, t("Sunburst Disk Analyser Help"));
                    for line in [
                        t("Pick a disk or folder in the list on the left, or drop one on the window, then click Start Scanning."),
                        t("Folders Outline lists the content by size. Click a folder to open it in the next column; click a file to preview it, double-click it to open it. Scroll sideways to see the earlier columns, or use the arrow keys."),
                        t("In the chart every ring is one folder level. Hover a segment for details, click it to select it, click the centre to go back up."),
                        t("Biggest Files lists the largest files of the whole scan."),
                        t("Tick items, then use the Review button to move them to the Trash, or to move or copy them to another folder."),
                        t("Right-click any item for more actions."),
                    ] {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("•").color(pal.accent_hover));
                            ui.add(egui::Label::new(line).wrap());
                        });
                    }
                    buttons(ui, |ui| {
                        if primary(ui, t("Close"), pal.accent) {
                            outcome = Outcome::Close;
                        }
                    });
                }
                Dialog::Message { title: heading, body } => {
                    ui.set_width(420.0);
                    title(ui, heading);
                    ui.add(egui::Label::new(RichText::new(body.as_str()).color(pal.text_dim)).wrap());
                    buttons(ui, |ui| {
                        if primary(ui, t("OK"), pal.accent) {
                            outcome = Outcome::Close;
                        }
                    });
                }
            }
        });

        if settings_changed {
            self.settings.save();
        }
        let closable = !matches!(self.dialog, Dialog::Progress { .. });
        if matches!(outcome, Outcome::Keep) && response.should_close() && closable {
            outcome = Outcome::Close;
        }
        match outcome {
            Outcome::Keep => {}
            Outcome::Close => self.dialog = Dialog::None,
            Outcome::Confirm => self.confirm_review(),
            Outcome::Reveal(path) => {
                if !demo {
                    platform::reveal(&path);
                }
            }
        }
    }
}
