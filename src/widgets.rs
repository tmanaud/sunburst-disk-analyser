//! Custom-painted building blocks shared by all views.

use crate::session::Check;
use crate::theme::{self, Palette};
use crate::{icons, util};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui};

/// Rounded rectangle filled with a vertical gradient.
pub fn gradient_panel(p: &Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32, line: Color32) {
    let lerp = |y: f32| -> Color32 {
        let t = ((y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color32::from_rgb(mix(top.r(), bottom.r()), mix(top.g(), bottom.g()), mix(top.b(), bottom.b()))
    };
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut outline: Vec<Pos2> = Vec::with_capacity(40);
    for (c, start) in corners {
        for i in 0..=8 {
            let a = (start + i as f32 * 11.25).to_radians();
            outline.push(pos2(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), lerp(rect.center().y));
    for pt in &outline {
        mesh.colored_vertex(*pt, lerp(pt.y));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(mesh));
    if line != Color32::TRANSPARENT {
        p.add(Shape::closed_line(outline, Stroke::new(1.0_f32, line)));
    }
}

pub fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)).size().x
}

/// Middle-truncates `text` so it fits in `max_width`.
pub fn fit_middle(ui: &Ui, text: &str, font: &FontId, max_width: f32) -> String {
    if text_width(ui, text, font) <= max_width {
        return text.to_owned();
    }
    let total = text.chars().count();
    let (mut lo, mut hi) = (5usize, total.saturating_sub(1).max(5));
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if text_width(ui, &util::middle_ellipsis(text, mid), font) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    util::middle_ellipsis(text, lo)
}

/// Truncates at the end with an ellipsis.
pub fn fit_end(ui: &Ui, text: &str, font: &FontId, max_width: f32) -> String {
    if text_width(ui, text, font) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (1usize, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s: String = chars[..mid].iter().chain(std::iter::once(&'…')).collect();
        if text_width(ui, &s, font) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().chain(std::iter::once(&'…')).collect()
}

pub fn checkbox(p: &Painter, rect: Rect, check: Check, pal: &Palette, enabled: bool) {
    let rad = CornerRadius::same(4);
    match check {
        Check::No => {
            let line = if enabled { pal.control_line } else { pal.control_line.gamma_multiply(0.45) };
            p.rect_filled(rect, rad, if pal.dark { Color32::from_black_alpha(40) } else { Color32::WHITE });
            p.rect_stroke(rect, rad, Stroke::new(1.2_f32, line), StrokeKind::Inside);
        }
        Check::Yes => {
            p.rect_filled(rect, rad, pal.accent);
            icons::check(p, rect, Color32::WHITE, 1.8);
        }
        Check::Inherited => {
            p.rect_filled(rect, rad, pal.accent.gamma_multiply(0.45));
            icons::check(p, rect, Color32::from_white_alpha(170), 1.8);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Primary,
    Outline,
}

/// Rounded button painted in `rect`.
pub fn button(ui: &mut Ui, rect: Rect, text: &str, style: Btn, enabled: bool, pal: &Palette, font: FontId) -> Response {
    let id = ui.id().with(("btn", text, rect.min.x as i32, rect.min.y as i32));
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let rad = CornerRadius::same((rect.height() * 0.28).min(12.0) as u8);
    let hovered = enabled && resp.hovered();
    let (fill, line, fg) = match style {
        Btn::Primary if enabled => (if hovered { pal.accent_hover } else { pal.accent }, Color32::TRANSPARENT, Color32::WHITE),
        Btn::Outline if enabled => (if hovered { pal.row_hover } else { Color32::TRANSPARENT }, pal.control_line, pal.text),
        _ => (pal.control.gamma_multiply(0.85), pal.control_line.gamma_multiply(0.5), pal.text_faint),
    };
    p.rect_filled(rect, rad, fill);
    if line != Color32::TRANSPARENT {
        p.rect_stroke(rect, rad, Stroke::new(1.0_f32, line), StrokeKind::Inside);
    }
    let label = fit_end(ui, text, &font, rect.width() - 16.0);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Round icon button used in the toolbars. Returns the response and the icon rectangle.
pub fn round_button(ui: &mut Ui, center: Pos2, radius: f32, key: &str, pal: &Palette, active: bool) -> (Response, Rect) {
    let rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
    let resp = ui.interact(rect, ui.id().with(("round", key)), Sense::click());
    let fill = if resp.hovered() || active { pal.row_selected } else { pal.control.gamma_multiply(if pal.dark { 0.9 } else { 1.0 }) };
    ui.painter().circle_filled(center, radius, fill);
    ui.painter().circle_stroke(center, radius, Stroke::new(1.0_f32, pal.control_line));
    (resp, Rect::from_center_size(center, vec2(radius * 1.05, radius * 1.05)))
}

pub fn spinner(p: &Painter, center: Pos2, radius: f32, color: Color32, time: f64) {
    let start = (time * 4.5) as f32;
    let pts: Vec<Pos2> = (0..=18)
        .map(|i| {
            let a = start + i as f32 * 0.24;
            pos2(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(1.8_f32, color)));
}

/// Segmented control with two or more labels; returns the clicked index.
pub fn segmented(ui: &mut Ui, rect: Rect, labels: &[&str], selected: usize, pal: &Palette) -> Option<usize> {
    let rad = CornerRadius::same((rect.height() * 0.5) as u8);
    ui.painter().rect_filled(rect, rad, pal.control.gamma_multiply(0.75));
    ui.painter().rect_stroke(rect, rad, Stroke::new(1.0_f32, pal.control_line.gamma_multiply(0.7)), StrokeKind::Inside);
    let w = rect.width() / labels.len() as f32;
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(rect.left() + w * i as f32, rect.top()), vec2(w, rect.height())).shrink(3.0);
        let resp = ui.interact(cell, ui.id().with(("seg", i)), Sense::click());
        let cell_rad = CornerRadius::same((cell.height() * 0.5) as u8);
        if i == selected {
            ui.painter().rect_filled(cell, cell_rad, pal.side_selected);
        } else if resp.hovered() {
            ui.painter().rect_filled(cell, cell_rad, pal.row_hover);
        }
        let font = if i == selected { theme::semibold(13.5) } else { theme::regular(13.5) };
        let text = fit_end(ui, label, &font, cell.width() - 10.0);
        ui.painter().text(cell.center(), Align2::CENTER_CENTER, text, font, if i == selected { pal.text } else { pal.text_dim });
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Small rounded popover with a title and label/value lines, placed near `anchor`.
pub fn info_popover(ctx: &egui::Context, anchor: Pos2, bounds: Rect, title: &str, lines: &[(String, String)], pal: &Palette) {
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("chart-popover"));
    let p = ctx.layer_painter(layer);
    let title_font = theme::semibold(13.0);
    let font = theme::regular(12.5);
    let measure = |t: &str, f: &FontId| ctx.fonts(|fonts| fonts.layout_no_wrap(t.to_owned(), f.clone(), Color32::WHITE)).size().x;
    let title: String = if title.chars().count() > 38 { util::middle_ellipsis(title, 38) } else { title.to_owned() };
    let label_w = lines.iter().map(|l| measure(&l.0, &font)).fold(0.0, f32::max);
    let value_w = lines.iter().map(|l| measure(&l.1, &font)).fold(0.0, f32::max);
    let w = (label_w + value_w + 18.0).max(measure(&title, &title_font)) + 28.0;
    let h = 36.0 + lines.len() as f32 * 19.0 + 6.0;
    let mut min = anchor + vec2(14.0, -h * 0.5);
    if min.x + w > bounds.right() - 4.0 {
        min.x = anchor.x - 14.0 - w;
    }
    min.x = min.x.max(bounds.left() + 4.0);
    min.y = min.y.clamp(bounds.top() + 4.0, (bounds.bottom() - h - 4.0).max(bounds.top() + 4.0));
    let rect = Rect::from_min_size(min, vec2(w, h));
    p.rect_filled(rect.translate(vec2(0.0, 3.0)).expand(1.0), CornerRadius::same(9), Color32::from_black_alpha(60));
    p.rect_filled(rect, CornerRadius::same(8), pal.popup);
    p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0_f32, pal.popup_line), StrokeKind::Inside);
    p.text(pos2(rect.center().x, rect.top() + 18.0), Align2::CENTER_CENTER, title, title_font, pal.text);
    for (i, (label, value)) in lines.iter().enumerate() {
        let y = rect.top() + 44.0 + i as f32 * 19.0;
        p.text(pos2(rect.left() + 14.0, y), Align2::LEFT_CENTER, label, font.clone(), pal.text_dim);
        p.text(pos2(rect.left() + 14.0 + label_w + 18.0, y), Align2::LEFT_CENTER, value, font.clone(), pal.text);
    }
}

/// Paints the icon of a file system item in `rect`.
pub fn item_icon(p: &Painter, rect: Rect, flags: u8, pal: &Palette, dim: bool) {
    use crate::tree::{F_DIR, F_PACKAGE};
    let fade = |c: Color32| if dim { c.gamma_multiply(0.5) } else { c };
    if flags & F_PACKAGE != 0 {
        icons::package(p, rect, fade(pal.accent));
    } else if flags & F_DIR != 0 {
        icons::folder(p, rect, fade(pal.folder), fade(pal.folder_tab));
    } else {
        icons::file(p, rect, fade(pal.file), if pal.dark { Color32::from_rgb(90, 104, 130) } else { Color32::from_rgb(120, 134, 160) });
        if !pal.dark {
            // A white page needs an edge on a light background.
            let r = Rect::from_min_max(
                pos2(rect.left() + rect.width() * 0.2, rect.top() + rect.height() * 0.06),
                pos2(rect.left() + rect.width() * 0.8, rect.top() + rect.height() * 0.94),
            );
            p.rect_stroke(r, CornerRadius::same(1), Stroke::new(0.8_f32, pal.control_line), StrokeKind::Inside);
        }
    }
}
