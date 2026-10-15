#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod chart;
mod demo;
mod dialogs;
mod export;
mod filter;
mod i18n;
mod icon;
mod icons;
#[cfg(target_os = "macos")]
mod menu;
mod ops;
#[cfg(unix)]
mod pinned;
mod platform;
mod record;
mod scan;
mod session;
mod settings;
mod swipe;
mod theme;
mod thumb;
mod tree;
mod util;
mod views;
mod widgets;

use std::path::PathBuf;

const USAGE: &str = "Sunburst Disk Analyser

Usage: sunburst-disk-analyser [OPTIONS] [FOLDER]

  FOLDER                      Folder or disk to scan right away
  --lang <code>               Interface language: en fr de es it ru uk zh
  --theme <light|dark>        Appearance for this run
  --demo                      Show built-in sample data instead of your disk
  --screenshot <scene> <png>  Render one scene to a PNG file and quit
  --record <folder>           Record the README animation as PNG frames and quit
  --demo-media <folder>       Pictures of the demo files (assets/demo)
  --size <WxH>                Window size in points
  --render-icon <png> [size]  Write the application icon and quit
  -h, --help                  Show this help
  -V, --version               Show the version";

fn main() -> eframe::Result<()> {
    let mut opts = app::Options::default();
    let mut size = [1180.0f32, 780.0f32];
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--demo" => opts.demo = true,
            "--lang" => opts.lang = args.next(),
            "--theme" => opts.theme = args.next(),
            "--screenshot" => {
                if let (Some(scene), Some(out)) = (args.next(), args.next()) {
                    opts.shot = Some((scene, PathBuf::from(out)));
                    opts.demo = true;
                }
            }
            "--record" => {
                if let Some(out) = args.next() {
                    opts.record = Some(PathBuf::from(out));
                    opts.demo = true;
                }
            }
            "--demo-media" => opts.media = args.next().map(PathBuf::from),
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(|s| s.split_once('x')) {
                    if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                        size = [w, h];
                    }
                }
            }
            "--render-icon" => {
                let Some(out) = args.next() else { return Ok(()) };
                let px = args.next().and_then(|s| s.parse().ok()).unwrap_or(1024);
                if let Err(e) = icon::render(px).save(&out) {
                    eprintln!("could not write {out}: {e}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            other if !other.starts_with('-') => opts.scan = Some(PathBuf::from(other)),
            other => {
                eprintln!("Unknown option: {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    let icon = icon::render(256);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Sunburst Disk Analyser")
        .with_app_id("sunburst-disk-analyser")
        .with_inner_size(size)
        .with_min_inner_size([780.0, 520.0])
        .with_drag_and_drop(true)
        .with_icon(egui::IconData { width: icon.width(), height: icon.height(), rgba: icon.into_raw() });
    if cfg!(target_os = "macos") {
        // Content runs under a transparent title bar, as in native macOS utilities.
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    let options = eframe::NativeOptions { viewport, multisampling: 4, ..Default::default() };
    eframe::run_native(
        "Sunburst Disk Analyser",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, opts)))),
    )
}
