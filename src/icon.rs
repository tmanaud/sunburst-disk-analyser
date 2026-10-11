//! The application icon, rendered procedurally so no binary asset is needed.

use image::{Rgba, RgbaImage};

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match (i as i32).rem_euclid(6) {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Colour and coverage of the icon at a point of the unit square.
fn sample(x: f32, y: f32) -> ([f32; 3], f32) {
    // Rounded square with a small transparent margin.
    let (m, rad) = (0.06, 0.2);
    let (cx, cy) = ((x - 0.5).abs(), (y - 0.5).abs());
    let half = 0.5 - m;
    let (dx, dy) = ((cx - (half - rad)).max(0.0), (cy - (half - rad)).max(0.0));
    if cx > half || cy > half || dx * dx + dy * dy > rad * rad {
        return ([0.0; 3], 0.0);
    }
    let mut c = [0.13 - 0.05 * y, 0.24 - 0.08 * y, 0.44 - 0.14 * y];
    let (px, py) = (x - 0.5, y - 0.52);
    let r = (px * px + py * py).sqrt();
    let mut a = px.atan2(-py) / std::f32::consts::TAU;
    if a < 0.0 {
        a += 1.0;
    }
    // Outer extent of each of the 24 sectors, in rings.
    const DEPTH: [u8; 24] = [3, 3, 2, 3, 1, 2, 2, 1, 3, 2, 1, 1, 2, 3, 3, 1, 2, 1, 1, 2, 3, 2, 1, 2];
    let sector = (a * 24.0) as usize % 24;
    let (r0, ring) = (0.16, 0.082);
    if r < r0 - 0.012 {
        let t = (r / r0).powi(2);
        c = [0.24 - 0.08 * t, 0.50 - 0.14 * t, 0.86 - 0.12 * t];
    } else if r < r0 {
        c = [0.60, 0.76, 0.98];
    } else {
        let depth = ((r - r0) / ring) as u8;
        let edge = (a * 24.0).fract();
        let gap = (r - r0) / ring - depth as f32;
        if depth < DEPTH[sector] && gap > 0.07 && (depth == 0 || (edge > 0.05 && edge < 0.95)) {
            let rgb = hsv((a * 0.8).fract(), 0.72 - 0.06 * depth as f32, 0.98);
            c = rgb;
        }
    }
    (c, 1.0)
}

pub fn render(size: u32) -> RgbaImage {
    const SS: u32 = 4;
    let mut img = RgbaImage::new(size, size);
    for py in 0..size {
        for px in 0..size {
            let (mut rgb, mut alpha) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let (c, a) = sample(x, y);
                    for k in 0..3 {
                        rgb[k] += c[k] * a;
                    }
                    alpha += a;
                }
            }
            let n = (SS * SS) as f32;
            let px_color = if alpha > 0.0 {
                let to8 = |v: f32| (v / alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                Rgba([to8(rgb[0]), to8(rgb[1]), to8(rgb[2]), (alpha / n * 255.0).round() as u8])
            } else {
                Rgba([0, 0, 0, 0])
            };
            img.put_pixel(px, py, px_color);
        }
    }
    img
}
