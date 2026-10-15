//! Scripted recording of the README animation (`--record <folder>`).
//!
//! The app runs on the demo data with a clock of exactly `FPS` frames a
//! second, whatever time the frames really take. A drawn pointer clicks
//! through the real interface, and every frame is saved as a PNG inside a
//! drawn window frame, ready to be encoded into an animation.

use egui::{pos2, vec2, Color32, Event, Id, LayerId, Order, Pos2, Shape, Stroke};
use image::RgbaImage;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

pub const FPS: f64 = 30.0;
/// Frames drawn before the recording starts, while the first layouts settle.
const WARM_UP: u64 = 8;

/// Where the pointer goes.
pub enum At {
    /// A spot noted with [`mark`] while drawing the previous frame.
    Mark(String),
    /// A point given in shares of the window size.
    Share(f32, f32),
}

pub enum Step {
    Wait(f64),
    Move(At, f64),
    Click,
    /// Plays the scan started by the last click back over that many seconds,
    /// while the next steps go on.
    Scan(f64),
    WaitScan,
    /// Switches to the dark appearance with a cross-fade that long.
    Dark(f64),
}

/// What the app has to do for the script.
pub enum Effect {
    /// Show that share of the scan.
    Scan(f64),
    ScanDone,
    Dark,
}

/// The README animation: start a scan, open a folder, preview a film,
/// switch to the dark appearance and end on the biggest files.
pub fn script() -> Vec<Step> {
    use At::*;
    use Step::*;
    vec![
        Wait(0.35),
        Move(Mark("scan".into()), 0.6),
        Wait(0.1),
        Click,
        Scan(2.0),
        Move(Share(0.66, 0.45), 1.0),
        WaitScan,
        Wait(0.2),
        Move(Mark("row:0:Movies".into()), 0.6),
        Wait(0.08),
        Click,
        Wait(0.45),
        Move(Mark(format!("seg:{}", crate::demo::MOVIE)), 0.7),
        Wait(0.45),
        Click,
        Wait(1.0),
        Dark(0.5),
        Wait(0.15),
        Move(Mark("tab:biggest".into()), 0.55),
        Wait(0.08),
        Click,
        Wait(0.2),
        // Off the tab, so that the last frame shows it whole.
        Move(Share(0.6, 0.046), 0.4),
        Wait(1.0),
    ]
}

// ---- marks --------------------------------------------------------------------

#[derive(Clone, Default)]
struct Marks(HashMap<String, Pos2>);

fn marks_id() -> Id {
    Id::new("record-marks")
}

fn active_id() -> Id {
    Id::new("record-active")
}

/// Whether a recording is running: the views then note where things are.
pub fn active(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(active_id())).unwrap_or(false)
}

/// Notes where something the script may point at was drawn.
pub fn mark(ctx: &egui::Context, key: impl FnOnce() -> String, pos: Pos2) {
    if active(ctx) {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Marks>(marks_id()).0.insert(key(), pos));
    }
}

// ---- recorder -----------------------------------------------------------------

/// How a saved frame is made from the screenshot of a frame.
#[derive(Clone, Copy)]
enum Tag {
    Plain,
    /// Mixed with the last frame before the fade, this much of the new one.
    Fade(f32),
}

pub struct Recorder {
    steps: Vec<Step>,
    step: usize,
    step_start: f64,
    frame: u64,
    pointer: Pos2,
    from: Pos2,
    pressed: bool,
    click_at: Option<(f64, Pos2)>,
    scan: Option<(f64, f64)>,
    scan_done: bool,
    fade: Option<(f64, f64)>,
    /// Tags of the screenshots asked for and not received yet.
    tags: VecDeque<Tag>,
    prev: Option<RgbaImage>,
    base: Option<RgbaImage>,
    saved: u64,
    writer: Option<Writer>,
    out: PathBuf,
}

impl Recorder {
    pub fn new(ctx: &egui::Context, out: PathBuf) -> Recorder {
        ctx.data_mut(|d| d.insert_temp(active_id(), true));
        Recorder {
            steps: script(),
            step: 0,
            step_start: 0.0,
            frame: 0,
            pointer: Pos2::ZERO,
            from: Pos2::ZERO,
            pressed: false,
            click_at: None,
            scan: None,
            scan_done: false,
            fade: None,
            tags: VecDeque::new(),
            prev: None,
            base: None,
            saved: 0,
            writer: None,
            out,
        }
    }

    /// Script time, in seconds since the recording started.
    fn time(&self) -> f64 {
        self.frame.saturating_sub(WARM_UP) as f64 / FPS
    }

    fn recording(&self) -> bool {
        self.frame > WARM_UP && self.step < self.steps.len()
    }

    /// Replaces the real input of the frame about to be drawn by the script.
    pub fn input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) -> Vec<Effect> {
        self.frame += 1;
        raw.events.retain(|e| matches!(e, Event::Screenshot { .. }));
        raw.hovered_files.clear();
        raw.dropped_files.clear();
        raw.modifiers = Default::default();
        raw.time = Some(self.frame as f64 / FPS);
        raw.predicted_dt = (1.0 / FPS) as f32;
        let marks = ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Marks>(marks_id())));
        let screen = ctx.screen_rect();
        let at = |a: &At| match a {
            At::Mark(key) => marks.0.get(key).copied(),
            At::Share(x, y) => Some(pos2(screen.width() * x, screen.height() * y)),
        };
        if self.frame <= WARM_UP {
            self.pointer = pos2(screen.width() * 0.62, screen.height() * 0.55);
            raw.events.push(Event::PointerMoved(self.pointer));
            return Vec::new();
        }

        let t = self.time();
        let mut effects = Vec::new();
        while let Some(step) = self.steps.get(self.step) {
            let elapsed = t - self.step_start;
            let done = match step {
                Step::Wait(d) => elapsed >= *d,
                Step::Move(target, d) => match at(target) {
                    // A target not drawn yet is waited for.
                    None => false,
                    Some(to) => {
                        let x = (elapsed / d).clamp(0.0, 1.0) as f32;
                        let e = x * x * (3.0 - 2.0 * x);
                        self.pointer = self.from + (to - self.from) * e;
                        x >= 1.0
                    }
                },
                Step::Click => {
                    if !self.pressed {
                        self.pressed = true;
                        self.click_at = Some((t, self.pointer));
                        raw.events.push(button(self.pointer, true));
                        false
                    } else if elapsed >= 0.06 {
                        self.pressed = false;
                        raw.events.push(button(self.pointer, false));
                        true
                    } else {
                        false
                    }
                }
                Step::Scan(d) => {
                    self.scan = Some((t, *d));
                    true
                }
                Step::WaitScan => self.scan_done,
                Step::Dark(d) => {
                    if self.fade.is_none() {
                        self.fade = Some((t, *d));
                        effects.push(Effect::Dark);
                    }
                    elapsed >= *d
                }
            };
            if !done {
                break;
            }
            self.step += 1;
            self.step_start = t;
            self.from = self.pointer;
        }
        raw.events.push(Event::PointerMoved(self.pointer));

        // The scan shows a little more ten times a second, as a real one does.
        if let Some((start, d)) = self.scan.filter(|_| !self.scan_done) {
            let x = ((t - start) / d).min(1.0);
            if x >= 1.0 {
                self.scan_done = true;
                effects.push(Effect::ScanDone);
            } else if self.frame.is_multiple_of(3) {
                effects.push(Effect::Scan(1.0 - (1.0 - x) * (1.0 - x)));
            }
        }
        effects
    }

    /// Draws the pointer over the frame and saves the frames received.
    pub fn after_frame(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
        self.paint_pointer(ctx);
        if self.recording() {
            let t = self.time();
            let tag = match self.fade {
                Some((start, d)) if t < start + d => Tag::Fade(((t - start) / d) as f32),
                _ => Tag::Plain,
            };
            self.tags.push_back(tag);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }

        let shots: Vec<Arc<egui::ColorImage>> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        for shot in shots {
            let Some(tag) = self.tags.pop_front() else { continue };
            let [w, h] = shot.size;
            let Some(mut img) = RgbaImage::from_raw(w as u32, h as u32, shot.as_raw().to_vec()) else { continue };
            match tag {
                Tag::Fade(a) => {
                    if self.base.is_none() {
                        self.base = self.prev.take();
                    }
                    if let Some(base) = &self.base {
                        mix(&mut img, base, a);
                    }
                }
                Tag::Plain => self.base = None,
            }
            let ppp = w as f32 / ctx.screen_rect().width();
            let writer = self.writer.get_or_insert_with(|| Writer::new(self.out.clone(), ppp));
            self.saved += 1;
            writer.save(self.saved, img.clone());
            self.prev = Some(img);
        }

        if !self.recording() && self.frame > WARM_UP && self.tags.is_empty() {
            if let Some(writer) = self.writer.take() {
                writer.finish();
                println!("{} frames written to {}", self.saved, self.out.display());
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// A macOS-like arrow, with a ring after a click.
    fn paint_pointer(&self, ctx: &egui::Context) {
        let p = ctx.layer_painter(LayerId::new(Order::Debug, Id::new("record-pointer")));
        let t = self.time();
        if let Some((at, pos)) = self.click_at {
            let x = ((t - at) / 0.35) as f32;
            if (0.0..1.0).contains(&x) {
                let alpha = ((1.0 - x) * 150.0) as u8;
                p.circle_stroke(pos, 7.0 + 13.0 * x, Stroke::new(2.5_f32, Color32::from_rgba_unmultiplied(40, 120, 255, alpha)));
            }
        }
        let s = 1.15;
        let at = |x: f32, y: f32| self.pointer + vec2(x, y) * s;
        let outline = vec![at(0.0, 0.0), at(0.0, 17.0), at(4.0, 13.2), at(7.0, 19.6), at(9.6, 18.5), at(6.8, 12.3), at(12.2, 12.3)];
        p.add(Shape::closed_line(outline, Stroke::new(3.0_f32, Color32::WHITE)));
        let mut mesh = egui::Mesh::default();
        let mut tri = |a: Pos2, b: Pos2, c: Pos2| {
            let i = mesh.vertices.len() as u32;
            for v in [a, b, c] {
                mesh.colored_vertex(v, Color32::BLACK);
            }
            mesh.add_triangle(i, i + 1, i + 2);
        };
        tri(at(0.0, 0.0), at(0.0, 17.0), at(4.0, 13.2));
        tri(at(0.0, 0.0), at(4.0, 13.2), at(6.8, 12.3));
        tri(at(0.0, 0.0), at(6.8, 12.3), at(12.2, 12.3));
        tri(at(4.0, 13.2), at(7.0, 19.6), at(9.6, 18.5));
        tri(at(4.0, 13.2), at(9.6, 18.5), at(6.8, 12.3));
        p.add(Shape::mesh(mesh));
    }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// `img` becomes `base` mixed with `a` of `img`.
fn mix(img: &mut RgbaImage, base: &RgbaImage, a: f32) {
    if img.dimensions() != base.dimensions() {
        return;
    }
    for (px, b) in img.pixels_mut().zip(base.pixels()) {
        for c in 0..4 {
            px.0[c] = (b.0[c] as f32 + (px.0[c] as f32 - b.0[c] as f32) * a).round() as u8;
        }
    }
}

// ---- saving -------------------------------------------------------------------

/// Frames the pictures and writes them as PNG on a few threads.
struct Writer {
    tx: Option<mpsc::SyncSender<(u64, RgbaImage)>>,
    threads: Vec<JoinHandle<()>>,
}

impl Writer {
    fn new(dir: PathBuf, ppp: f32) -> Writer {
        let (tx, rx) = mpsc::sync_channel::<(u64, RgbaImage)>(6);
        let rx = Arc::new(Mutex::new(rx));
        let threads = (0..4)
            .map(|_| {
                let (rx, dir) = (rx.clone(), dir.clone());
                std::thread::spawn(move || loop {
                    let job = rx.lock().map(|r| r.recv());
                    let Ok(Ok((n, img))) = job else { break };
                    let out = dir.join(format!("frame-{n:04}.png"));
                    if let Err(e) = save_png(&window_frame(&img, ppp), &out, true) {
                        eprintln!("could not write {}: {e}", out.display());
                    }
                })
            })
            .collect();
        Writer { tx: Some(tx), threads }
    }

    fn save(&self, n: u64, img: RgbaImage) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((n, img));
        }
    }

    fn finish(mut self) {
        self.tx = None;
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

pub fn save_png(img: &RgbaImage, path: &Path, fast: bool) -> Result<(), String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let compression = if fast { CompressionType::Fast } else { CompressionType::Best };
    PngEncoder::new_with_quality(std::io::BufWriter::new(file), compression, FilterType::Adaptive)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}

// ---- window frame -------------------------------------------------------------

/// Margin around the window for its shadow, in points.
const MARGIN: f32 = 40.0;
const CORNER: f32 = 12.0;

/// What is drawn around and over the window, for one picture size.
struct Chrome {
    size: (u32, u32),
    ppp_bits: u32,
    margin: u32,
    /// Shadow opacity over the whole canvas.
    shadow: Vec<u8>,
    /// Share of each window pixel inside the rounded corners.
    cover: Vec<u8>,
    /// Opacity of the thin outline of the window.
    edge: Vec<u8>,
}

fn chrome(w: u32, h: u32, ppp: f32) -> Arc<Chrome> {
    static CACHE: OnceLock<Mutex<Option<Arc<Chrome>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.as_ref().filter(|c| c.size == (w, h) && c.ppp_bits == ppp.to_bits()) {
        return c.clone();
    }
    let margin = (MARGIN * ppp).round() as u32;
    let radius = CORNER * ppp;
    let (cw, ch) = (w + 2 * margin, h + 2 * margin);
    // Signed distance to the edge of the rounded window, negative inside.
    let dist = |x: f32, y: f32| {
        let qx = (x - w as f32 * 0.5).abs() - (w as f32 * 0.5 - radius);
        let qy = (y - h as f32 * 0.5).abs() - (h as f32 * 0.5 - radius);
        (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - radius
    };
    let mut cover = vec![0u8; (w * h) as usize];
    let mut edge = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let d = dist(x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * w + x) as usize;
            cover[i] = ((0.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
            edge[i] = ((1.0 - (d + ppp * 0.5).abs() / (ppp * 0.5)).clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    // A soft shadow, a little below the window.
    let drop = (10.0 * ppp) as i64;
    let mut shadow = vec![0f32; (cw * ch) as usize];
    for y in 0..h {
        let cy = y as i64 + margin as i64 + drop;
        if cy < 0 || cy >= ch as i64 {
            continue;
        }
        for x in 0..w {
            shadow[(cy as u32 * cw + x + margin) as usize] = cover[(y * w + x) as usize] as f32 / 255.0;
        }
    }
    let r = (9.0 * ppp) as usize;
    for _ in 0..3 {
        blur(&mut shadow, cw as usize, ch as usize, r);
    }
    let shadow = shadow.iter().map(|v| (v * 0.38 * 255.0) as u8).collect();
    let c = Arc::new(Chrome { size: (w, h), ppp_bits: ppp.to_bits(), margin, shadow, cover, edge });
    *cache = Some(c.clone());
    c
}

/// Box blur of radius `r`, across then down.
fn blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    let mut line = Vec::new();
    let mut pass = |v: &mut [f32], n: usize, len: usize, at: &dyn Fn(usize, usize) -> usize| {
        for k in 0..n {
            line.clear();
            line.extend((0..len).map(|i| v[at(k, i)]));
            let mut sum: f32 = line.iter().take(r + 1).sum();
            let span = (2 * r + 1) as f32;
            for i in 0..len {
                v[at(k, i)] = sum / span;
                if i + r + 1 < len {
                    sum += line[i + r + 1];
                }
                if i >= r {
                    sum -= line[i - r];
                }
            }
        }
    };
    pass(v, h, w, &|k, i| k * w + i);
    pass(v, w, h, &|k, i| i * w + k);
}

/// The picture of the window content inside a macOS-like window: rounded
/// corners, window buttons, outline and shadow, on a transparent background.
pub fn window_frame(img: &RgbaImage, ppp: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let c = chrome(w, h, ppp);
    let m = c.margin;
    let mut out = RgbaImage::new(w + 2 * m, h + 2 * m);
    for (px, s) in out.pixels_mut().zip(&c.shadow) {
        px.0 = [0, 0, 0, *s];
    }
    let mut win = img.clone();
    paint_window_buttons(&mut win, ppp);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let cov = c.cover[i] as f32 / 255.0;
            if cov <= 0.0 {
                continue;
            }
            let mut rgb = win.get_pixel(x, y).0;
            let e = c.edge[i] as f32 / 255.0 * 0.35;
            for v in rgb.iter_mut().take(3) {
                *v = (*v as f32 * (1.0 - e) + 110.0 * e) as u8;
            }
            let dst = out.get_pixel_mut(x + m, y + m);
            let sa = dst.0[3] as f32 / 255.0;
            let a = cov + sa * (1.0 - cov);
            // Over a black shadow: the colour only comes from the window.
            for (d, v) in dst.0.iter_mut().zip(rgb).take(3) {
                *d = ((v as f32 * cov) / a.max(1e-6)).round().min(255.0) as u8;
            }
            dst.0[3] = (a * 255.0).round() as u8;
        }
    }
    out
}

/// The three macOS window buttons, which the system draws outside of the app.
fn paint_window_buttons(img: &mut RgbaImage, ppp: f32) {
    #[cfg(not(target_os = "macos"))]
    let _ = (img, ppp);
    #[cfg(target_os = "macos")]
    {
        let (left, cy) = crate::views::window_buttons_at();
        let colors = [[255, 95, 87], [254, 188, 46], [40, 200, 64]];
        for (i, col) in colors.iter().enumerate() {
            let centre = ((left + 7.0 + i as f32 * 20.0) * ppp, cy * ppp);
            let r = 6.0 * ppp;
            let (x0, x1) = ((centre.0 - r - 2.0) as u32, (centre.0 + r + 2.0) as u32);
            let (y0, y1) = ((centre.1 - r - 2.0) as u32, (centre.1 + r + 2.0) as u32);
            for y in y0..=y1.min(img.height() - 1) {
                for x in x0..=x1.min(img.width() - 1) {
                    let d = (x as f32 + 0.5 - centre.0).hypot(y as f32 + 0.5 - centre.1);
                    let cov = (r - d + 0.5).clamp(0.0, 1.0);
                    if cov <= 0.0 {
                        continue;
                    }
                    // A slightly darker rim, as on the real buttons.
                    let rim = ((d - (r - ppp * 0.6)) / (ppp * 0.6)).clamp(0.0, 1.0) * 0.18;
                    let px = img.get_pixel_mut(x, y);
                    for (v, c) in px.0.iter_mut().zip(col) {
                        let c = *c as f32 * (1.0 - rim);
                        *v = (*v as f32 * (1.0 - cov) + c * cov).round() as u8;
                    }
                }
            }
        }
    }
}
