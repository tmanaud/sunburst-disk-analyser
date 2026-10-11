//! Colours and fonts for the dark (default) and light appearances.

use egui::{Color32, FontFamily, FontId};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    pub side_top: Color32,
    pub side_bottom: Color32,
    pub panel_line: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub row_hover: Color32,
    pub row_selected: Color32,
    pub side_selected: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub control: Color32,
    pub control_line: Color32,
    pub size: [Color32; 4],
    pub center_a: Color32,
    pub center_b: Color32,
    pub center_ring: Color32,
    pub seg_hidden: Color32,
    pub seg_checked: Color32,
    pub seg_free: Color32,
    pub seg_other: Color32,
    pub popup: Color32,
    pub popup_line: Color32,
    pub bar: Color32,
    pub folder: Color32,
    pub folder_tab: Color32,
    pub file: Color32,
    pub danger: Color32,
}

impl Palette {
    pub fn dark() -> Palette {
        let c = Color32::from_rgb;
        Palette {
            dark: true,
            bg_top: c(27, 46, 84),
            bg_bottom: c(21, 37, 68),
            side_top: c(30, 76, 131),
            side_bottom: c(22, 53, 97),
            panel_line: Color32::from_rgba_unmultiplied(120, 160, 220, 60),
            text: c(238, 243, 252),
            text_dim: c(160, 180, 212),
            text_faint: c(112, 134, 170),
            row_hover: Color32::from_rgba_unmultiplied(120, 170, 255, 26),
            row_selected: c(44, 92, 160),
            side_selected: c(48, 108, 176),
            accent: c(48, 122, 246),
            accent_hover: c(78, 144, 255),
            control: c(36, 62, 108),
            control_line: Color32::from_rgba_unmultiplied(140, 180, 240, 90),
            size: [c(172, 188, 214), c(232, 198, 88), c(240, 152, 92), c(242, 120, 112)],
            center_a: c(58, 122, 204),
            center_b: c(36, 86, 166),
            center_ring: c(126, 168, 236),
            seg_hidden: c(206, 222, 248),
            seg_checked: c(186, 240, 218),
            seg_free: Color32::from_rgba_unmultiplied(120, 160, 220, 40),
            seg_other: c(108, 122, 150),
            popup: c(27, 47, 86),
            popup_line: Color32::from_rgba_unmultiplied(140, 180, 240, 110),
            bar: c(18, 33, 62),
            folder: c(86, 172, 240),
            folder_tab: c(58, 140, 216),
            file: c(236, 240, 247),
            danger: c(232, 84, 80),
        }
    }

    pub fn light() -> Palette {
        let c = Color32::from_rgb;
        Palette {
            dark: false,
            bg_top: c(247, 249, 253),
            bg_bottom: c(234, 240, 249),
            side_top: c(222, 233, 248),
            side_bottom: c(206, 221, 243),
            panel_line: Color32::from_rgba_unmultiplied(60, 90, 140, 50),
            text: c(24, 36, 58),
            text_dim: c(84, 102, 132),
            text_faint: c(132, 146, 170),
            row_hover: Color32::from_rgba_unmultiplied(40, 110, 230, 22),
            row_selected: c(196, 218, 250),
            side_selected: c(170, 202, 246),
            accent: c(40, 112, 236),
            accent_hover: c(66, 134, 250),
            control: c(255, 255, 255),
            control_line: Color32::from_rgba_unmultiplied(50, 90, 160, 160),
            size: [c(98, 112, 138), c(158, 122, 10), c(206, 108, 28), c(204, 62, 54)],
            center_a: c(92, 152, 232),
            center_b: c(58, 118, 208),
            center_ring: c(255, 255, 255),
            seg_hidden: c(196, 206, 224),
            seg_checked: c(150, 220, 190),
            seg_free: Color32::from_rgba_unmultiplied(60, 100, 170, 30),
            seg_other: c(150, 160, 180),
            popup: c(255, 255, 255),
            popup_line: Color32::from_rgba_unmultiplied(60, 100, 170, 90),
            bar: c(224, 232, 245),
            folder: c(78, 164, 236),
            folder_tab: c(52, 132, 210),
            file: c(255, 255, 255),
            danger: c(212, 60, 56),
        }
    }
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// Fonts shipped with the operating system that cover CJK and other scripts
/// missing from the bundled Latin/Cyrillic font.
fn system_fallback_fonts() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
        ]
    } else if cfg!(windows) {
        vec!["C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\simsun.ttc", "C:\\Windows\\Fonts\\malgun.ttf"]
    } else {
        vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ]
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf"))),
    );
    let mut fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(bytes) = system_fallback_fonts().into_iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("system-fallback".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fallback.push("system-fallback".into());
    }
    let mut regular = vec!["inter".to_string()];
    regular.extend(fallback.iter().cloned());
    let mut bold = vec!["inter-semibold".to_string()];
    bold.extend(fallback.iter().cloned());
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts.families.insert(FontFamily::Name("semibold".into()), bold);
    ctx.set_fonts(fonts);
}

pub fn apply_visuals(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.override_text_color = Some(p.text);
    v.window_fill = p.popup;
    v.panel_fill = p.bg_top;
    v.window_stroke = egui::Stroke::new(1.0_f32, p.popup_line);
    v.window_corner_radius = egui::CornerRadius::same(12);
    v.menu_corner_radius = egui::CornerRadius::same(8);
    v.selection.bg_fill = p.accent;
    v.selection.stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = p.accent_hover;
    v.extreme_bg_color = if p.dark { Color32::from_rgb(16, 30, 56) } else { Color32::WHITE };
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(6);
        w.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    }
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.panel_line);
    let idle = if p.dark { p.control } else { Color32::from_rgb(226, 234, 247) };
    v.widgets.inactive.weak_bg_fill = idle;
    v.widgets.inactive.bg_fill = idle;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.control_line);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.accent);
    v.widgets.hovered.weak_bg_fill = p.row_selected;
    v.widgets.hovered.bg_fill = p.row_selected;
    v.widgets.active.weak_bg_fill = p.accent;
    v.widgets.active.bg_fill = p.accent;
    v.widgets.open.weak_bg_fill = p.row_selected;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.interaction.tooltip_delay = 0.6;
        for (style, font) in [
            (egui::TextStyle::Body, regular(13.5)),
            (egui::TextStyle::Button, regular(13.5)),
            (egui::TextStyle::Small, regular(11.5)),
            (egui::TextStyle::Heading, semibold(18.0)),
            (egui::TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ] {
            s.text_styles.insert(style, font);
        }
    });
}
