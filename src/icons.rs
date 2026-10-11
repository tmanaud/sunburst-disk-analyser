//! Small vector icons painted directly, so the app ships no icon assets.

use egui::{pos2, vec2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind};

fn at(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + r.width() * x, r.top() + r.height() * y)
}

fn sub(r: Rect, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(at(r, x0, y0), at(r, x1, y1))
}

pub fn folder(p: &Painter, r: Rect, body: Color32, tab: Color32) {
    let rad = CornerRadius::same((r.width() * 0.12).round() as u8);
    p.rect_filled(sub(r, 0.04, 0.14, 0.52, 0.5), rad, tab);
    p.rect_filled(sub(r, 0.04, 0.26, 0.96, 0.86), rad, body);
    p.rect_filled(sub(r, 0.04, 0.26, 0.96, 0.34), rad, tab.gamma_multiply(0.55));
}

pub fn file(p: &Painter, r: Rect, paper: Color32, line: Color32) {
    let (l, t, rt, b, fold) = (0.2, 0.06, 0.8, 0.94, 0.26);
    let pts = vec![at(r, l, t), at(r, rt - fold, t), at(r, rt, t + fold), at(r, rt, b), at(r, l, b)];
    p.add(Shape::convex_polygon(pts, paper, Stroke::NONE));
    p.add(Shape::convex_polygon(
        vec![at(r, rt - fold, t), at(r, rt, t + fold), at(r, rt - fold, t + fold)],
        line.gamma_multiply(0.55),
        Stroke::NONE,
    ));
    let s = Stroke::new((r.width() * 0.05).max(1.0), line.gamma_multiply(0.45));
    for y in [0.52, 0.64, 0.76] {
        p.line_segment([at(r, l + 0.12, y), at(r, rt - 0.12, y)], s);
    }
}

pub fn package(p: &Painter, r: Rect, c: Color32) {
    let rad = CornerRadius::same((r.width() * 0.22).round() as u8);
    p.rect_filled(sub(r, 0.1, 0.1, 0.9, 0.9), rad, c);
    let s = Stroke::new((r.width() * 0.07).max(1.0), Color32::from_white_alpha(210));
    p.line_segment([at(r, 0.32, 0.68), at(r, 0.5, 0.3)], s);
    p.line_segment([at(r, 0.68, 0.68), at(r, 0.5, 0.3)], s);
    p.line_segment([at(r, 0.38, 0.56), at(r, 0.62, 0.56)], s);
}

pub fn group_dot(p: &Painter, r: Rect, c: Color32) {
    p.circle_filled(r.center(), r.width() * 0.3, c);
}

pub fn home(p: &Painter, r: Rect, c: Color32) {
    p.add(Shape::convex_polygon(vec![at(r, 0.5, 0.08), at(r, 0.98, 0.52), at(r, 0.02, 0.52)], c, Stroke::NONE));
    p.rect_filled(sub(r, 0.16, 0.46, 0.84, 0.92), CornerRadius::same(2), c);
    p.rect_filled(sub(r, 0.72, 0.14, 0.84, 0.4), CornerRadius::ZERO, c);
}

pub fn drive(p: &Painter, r: Rect, c: Color32, hole: Color32) {
    let rad = CornerRadius::same((r.width() * 0.16).round() as u8);
    p.rect_filled(sub(r, 0.06, 0.2, 0.94, 0.82), rad, c);
    p.rect_filled(sub(r, 0.14, 0.6, 0.86, 0.72), CornerRadius::same(2), hole);
}

pub fn desktop(p: &Painter, r: Rect, c: Color32) {
    p.rect_filled(sub(r, 0.06, 0.16, 0.94, 0.7), CornerRadius::same(3), c);
    p.rect_filled(sub(r, 0.3, 0.78, 0.7, 0.86), CornerRadius::same(1), c);
}

pub fn camera(p: &Painter, r: Rect, c: Color32, hole: Color32) {
    p.rect_filled(sub(r, 0.06, 0.28, 0.94, 0.82), CornerRadius::same(3), c);
    p.rect_filled(sub(r, 0.32, 0.16, 0.62, 0.32), CornerRadius::same(2), c);
    p.circle_filled(at(r, 0.5, 0.56), r.width() * 0.17, hole);
    p.circle_filled(at(r, 0.5, 0.56), r.width() * 0.09, c);
}

pub fn document(p: &Painter, r: Rect, c: Color32, hole: Color32) {
    file(p, r, c, hole);
}

pub fn download(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new((r.width() * 0.11).max(1.5), c);
    p.line_segment([at(r, 0.5, 0.12), at(r, 0.5, 0.62)], s);
    p.line_segment([at(r, 0.28, 0.42), at(r, 0.5, 0.64)], s);
    p.line_segment([at(r, 0.72, 0.42), at(r, 0.5, 0.64)], s);
    p.line_segment([at(r, 0.14, 0.86), at(r, 0.86, 0.86)], s);
}

pub fn star(p: &Painter, r: Rect, c: Color32) {
    let (cx, cy) = (r.center().x, r.center().y + r.height() * 0.03);
    let (ro, ri) = (r.width() * 0.46, r.width() * 0.2);
    let pts: Vec<Pos2> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rad = if i % 2 == 0 { ro } else { ri };
            pos2(cx + rad * a.cos(), cy + rad * a.sin())
        })
        .collect();
    // A star is concave: fan it out from the centre.
    for i in 0..10 {
        p.add(Shape::convex_polygon(vec![pos2(cx, cy), pts[i], pts[(i + 1) % 10]], c, Stroke::NONE));
    }
}

pub fn plus_circle(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.5_f32, c);
    p.circle_stroke(r.center(), r.width() * 0.44, s);
    p.line_segment([at(r, 0.5, 0.28), at(r, 0.5, 0.72)], s);
    p.line_segment([at(r, 0.28, 0.5), at(r, 0.72, 0.5)], s);
}

pub fn search(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.6_f32, c);
    p.circle_stroke(at(r, 0.44, 0.44), r.width() * 0.28, s);
    p.line_segment([at(r, 0.64, 0.64), at(r, 0.88, 0.88)], s);
}

pub fn sidebar(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.5_f32, c);
    let b = sub(r, 0.08, 0.18, 0.92, 0.82);
    p.rect_stroke(b, CornerRadius::same(3), s, StrokeKind::Inside);
    p.line_segment([at(r, 0.38, 0.18), at(r, 0.38, 0.82)], s);
}

pub fn chart(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.5_f32, c);
    p.circle_stroke(r.center(), r.width() * 0.42, s);
    p.line_segment([r.center(), at(r, 0.5, 0.08)], s);
    p.line_segment([r.center(), at(r, 0.2, 0.2)], s);
}

pub fn refresh(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.6_f32, c);
    let (ctr, rad) = (r.center(), r.width() * 0.36);
    let pts: Vec<Pos2> = (0..=22)
        .map(|i| {
            let a = -0.3 + i as f32 * 0.235;
            pos2(ctr.x + rad * a.cos(), ctr.y + rad * a.sin())
        })
        .collect();
    p.add(Shape::line(pts, s));
    let tip = pos2(ctr.x + rad * (-0.3f32).cos(), ctr.y + rad * (-0.3f32).sin());
    p.line_segment([tip, tip + vec2(-r.width() * 0.24, -r.width() * 0.02)], s);
    p.line_segment([tip, tip + vec2(r.width() * 0.04, -r.width() * 0.24)], s);
}

pub fn help(p: &Painter, r: Rect, c: Color32, font: egui::FontId) {
    p.text(r.center(), egui::Align2::CENTER_CENTER, "?", font, c);
}

pub fn gear(p: &Painter, r: Rect, c: Color32) {
    let ctr = r.center();
    let s = Stroke::new((r.width() * 0.14).max(1.5), c);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::FRAC_PI_4;
        let d = vec2(a.cos(), a.sin());
        p.line_segment([ctr + d * r.width() * 0.26, ctr + d * r.width() * 0.46], s);
    }
    p.circle_stroke(ctr, r.width() * 0.26, Stroke::new((r.width() * 0.12).max(1.5), c));
}

/// Chevron pointing right (`open == false`) or down.
pub fn chevron(p: &Painter, r: Rect, c: Color32, open: bool) {
    let s = Stroke::new(1.5_f32, c);
    if open {
        p.line_segment([at(r, 0.25, 0.38), at(r, 0.5, 0.64)], s);
        p.line_segment([at(r, 0.75, 0.38), at(r, 0.5, 0.64)], s);
    } else {
        p.line_segment([at(r, 0.38, 0.25), at(r, 0.64, 0.5)], s);
        p.line_segment([at(r, 0.38, 0.75), at(r, 0.64, 0.5)], s);
    }
}

pub fn check(p: &Painter, r: Rect, c: Color32, width: f32) {
    let s = Stroke::new(width, c);
    p.line_segment([at(r, 0.22, 0.52), at(r, 0.42, 0.72)], s);
    p.line_segment([at(r, 0.42, 0.72), at(r, 0.78, 0.3)], s);
}

pub fn warning(p: &Painter, r: Rect, c: Color32, mark: Color32) {
    p.add(Shape::convex_polygon(vec![at(r, 0.5, 0.1), at(r, 0.95, 0.88), at(r, 0.05, 0.88)], c, Stroke::NONE));
    p.line_segment([at(r, 0.5, 0.38), at(r, 0.5, 0.64)], Stroke::new(1.6_f32, mark));
    p.circle_filled(at(r, 0.5, 0.76), 1.1, mark);
}
