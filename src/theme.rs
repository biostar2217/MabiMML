//! 창 화면의 색 테마 (파이썬판 theme.py 와 같은 색), 글꼴, 아이콘.

use egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub wine: Color32,
    pub wine_dark: Color32,
    pub wine_soft: Color32,
    pub gold: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub hover: Color32,
    pub select: Color32,
    pub sub: Color32,
    pub icon_png: &'static [u8],
}

pub const OK: Color32 = Color32::from_rgb(0x3E, 0x8E, 0x5A);
pub const WARN: Color32 = Color32::from_rgb(0xD9, 0x8A, 0x1F);
pub const OVER: Color32 = Color32::from_rgb(0xC2, 0x3B, 0x3B);

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const THEME_NAMES: [&str; 2] = ["보라", "포도주"];
pub const DEFAULT_THEME: &str = "보라";

pub fn palette(name: &str) -> Palette {
    match name {
        "포도주" => Palette {
            bg: hex(0xF5EEE3),
            panel: hex(0xFFFBF4),
            wine: hex(0x6E1F2F),
            wine_dark: hex(0x521622),
            wine_soft: hex(0x8C3346),
            gold: hex(0xD9A441),
            ink: hex(0x2E2126),
            muted: hex(0x7A6A5F),
            line: hex(0xD8C9B3),
            hover: hex(0xF0E3CF),
            select: hex(0xE8C9A0),
            sub: hex(0xE8D3B5),
            icon_png: include_bytes!("../assets/icon_wine.png"),
        },
        _ => Palette {
            bg: hex(0xF4F2FF),
            panel: hex(0xFFFFFF),
            wine: hex(0x6A4CF5),
            wine_dark: hex(0x4B32C8),
            wine_soft: hex(0x8069F7),
            gold: hex(0xFFD36E),
            ink: hex(0x222222),
            muted: hex(0x6B6785),
            line: hex(0xD5CFF0),
            hover: hex(0xE9E5FF),
            select: hex(0xCFC6FF),
            sub: hex(0xE3DEFF),
            icon_png: include_bytes!("../assets/icon_64.png"),
        },
    }
}

/// 제목 띠 오른쪽 '남븝캘린더' 바로가기
pub const CALENDAR_NAME: &str = "남븝캘린더";
pub const CALENDAR_URL: &str = "https://nambebmabi.com/";
pub const CALENDAR_PNG: &[u8] = include_bytes!("../assets/calendar.png");

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::light();
    v.panel_fill = p.bg;
    v.window_fill = p.bg;
    v.extreme_bg_color = p.panel;
    v.faint_bg_color = p.hover;
    v.override_text_color = Some(p.ink);
    v.hyperlink_color = p.wine;
    v.selection.bg_fill = p.select;
    v.selection.stroke = egui::Stroke::new(1.0f32, p.ink);
    v.window_stroke = egui::Stroke::new(1.0f32, p.line);
    v.text_cursor.stroke = egui::Stroke::new(2.0f32, p.wine);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.weak_bg_fill = p.bg;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0f32, p.line);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0f32, p.ink);
    w.inactive.bg_fill = p.panel;
    w.inactive.weak_bg_fill = p.panel;
    w.inactive.bg_stroke = egui::Stroke::new(1.0f32, p.line);
    w.inactive.fg_stroke = egui::Stroke::new(1.0f32, p.ink);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = egui::Stroke::new(1.0f32, p.gold);
    w.hovered.fg_stroke = egui::Stroke::new(1.5f32, p.ink);
    w.active.bg_fill = p.select;
    w.active.weak_bg_fill = p.select;
    w.active.bg_stroke = egui::Stroke::new(1.0f32, p.gold);
    w.active.fg_stroke = egui::Stroke::new(1.5f32, p.ink);
    w.open.bg_fill = p.hover;
    w.open.weak_bg_fill = p.hover;
    w.open.bg_stroke = egui::Stroke::new(1.0f32, p.wine);
    for x in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        x.rounding = egui::Rounding::same(3.0f32);
    }
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.interact_size.y = 22.0;
    ctx.set_style(style);
}

/// 한글 글꼴: 시스템의 맑은 고딕. MML 칸은 Consolas (없으면 맑은 고딕).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let read = |name: &str| std::fs::read(std::path::Path::new(&windir).join("Fonts").join(name)).ok();
    if let Some(data) = read("malgun.ttf") {
        fonts.font_data.insert("malgun".into(), egui::FontData::from_owned(data));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "malgun".into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("malgun".into());
    }
    if let Some(data) = read("malgunbd.ttf") {
        fonts.font_data.insert("malgun_bold".into(), egui::FontData::from_owned(data));
        let mut fam = vec!["malgun_bold".to_string()];
        fam.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
        fonts.families.insert(egui::FontFamily::Name("bold".into()), fam);
    } else {
        let fam = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
        fonts.families.insert(egui::FontFamily::Name("bold".into()), fam);
    }
    if let Some(data) = read("consola.ttf") {
        fonts.font_data.insert("consolas".into(), egui::FontData::from_owned(data));
        fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "consolas".into());
    }
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style()).clone();
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(18.0, FontFamily::Name("bold".into()))),
        (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
    ]
    .into();
    ctx.set_style(style);
}

pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("bold".into()))
}

pub fn icon_data(png: &[u8]) -> Option<egui::IconData> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

pub fn color_image(png: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()))
}
