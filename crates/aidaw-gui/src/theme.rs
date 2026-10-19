//! The look of AIDAW: a dark "midnight studio" palette, Inter typography,
//! rounded widgets and a few painting helpers (glows, pills, the LCD).
//!
//! Every colour the editor paints by hand comes from here, so the whole app
//! can be re-skinned in one place.

use std::sync::Arc;

use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Margin, Painter, Rect, Stroke, TextStyle,
};

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

// ---- palette ----------------------------------------------------------------

/// Behind grids and timelines: the darkest layer.
pub const CANVAS: Color32 = rgb(0x0b, 0x0d, 0x13);
/// Panels (top bar, side bar, editors).
pub const PANEL: Color32 = rgb(0x12, 0x15, 0x1e);
/// The top bar and status bar sit a notch darker than the panels.
pub const CHROME: Color32 = rgb(0x0f, 0x11, 0x19);
/// Cards and inputs that sit on a panel.
pub const SURFACE: Color32 = rgb(0x1a, 0x1e, 0x2a);
/// Hovered cards, buttons.
pub const SURFACE_HI: Color32 = rgb(0x24, 0x29, 0x3a);
pub const BORDER: Color32 = rgb(0x26, 0x2b, 0x3b);
pub const BORDER_HI: Color32 = rgb(0x3a, 0x41, 0x58);

pub const TEXT: Color32 = rgb(0xe8, 0xea, 0xf2);
pub const TEXT_DIM: Color32 = rgb(0x8b, 0x92, 0xab);
pub const TEXT_FAINT: Color32 = rgb(0x5a, 0x61, 0x7a);

/// Brand accent: electric violet.
pub const ACCENT: Color32 = rgb(0x8b, 0x6c, 0xff);
/// Secondary accent: cyan, used for the LCD readouts.
pub const CYAN: Color32 = rgb(0x3d, 0xe0, 0xff);
pub const PLAY: Color32 = rgb(0x2e, 0xd5, 0x8e);
pub const PAUSE: Color32 = rgb(0xff, 0xa8, 0x3d);
pub const PLAYHEAD: Color32 = rgb(0xff, 0x4f, 0x8b);
pub const DANGER: Color32 = rgb(0xff, 0x5f, 0x6d);
pub const WARN: Color32 = rgb(0xff, 0xc8, 0x57);
pub const OK: Color32 = rgb(0x4c, 0xe0, 0x9a);

pub const EASY: Color32 = rgb(0x2e, 0xd5, 0x8e);
pub const PRO: Color32 = rgb(0x8b, 0x6c, 0xff);

pub const TRACK_COLORS: [Color32; 8] = [
    rgb(0x5b, 0x8c, 0xff),
    rgb(0xff, 0x8a, 0x3d),
    rgb(0x3d, 0xdc, 0x97),
    rgb(0xc8, 0x6b, 0xfa),
    rgb(0xff, 0xd2, 0x3f),
    rgb(0x2e, 0xd9, 0xff),
    rgb(0xff, 0x5c, 0x8a),
    rgb(0x9d, 0x8c, 0xff),
];

pub fn track_color(index: usize) -> Color32 {
    TRACK_COLORS[index % TRACK_COLORS.len()]
}

/// Linear mix of two colours (`t` = 0 → `a`, 1 → `b`), fully opaque.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Same hue, given alpha (0–255), unmultiplied.
pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

// ---- fonts --------------------------------------------------------------------

/// Semi-bold Inter, for headings, buttons and clip names.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, bold())
}

/// Some system fonts that carry CJK glyphs beyond the bundled subset
/// (Inter and egui's own fonts have none).
const CJK_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msyh.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/System/Library/Fonts/PingFang.ttc",
];

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Inter-Regular.otf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Inter-SemiBold.otf"
        ))),
    );
    // egui's own fonts stay as fallbacks: they carry the emoji and symbols.
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "inter".into());
    let mut b = vec!["inter-semibold".to_string()];
    b.extend(fallback);
    fonts.families.insert(bold(), b);
    // Bundled Simplified Chinese (all of GB2312 plus every character the
    // translations use), so the Chinese interface renders on any PC.
    fonts.font_data.insert(
        "noto-sc".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/NotoSansSC-Regular-subset.otf"
        ))),
    );
    // Right after the Latin font, so Chinese punctuation (，。！？) comes
    // from it rather than from the emoji fonts.
    for family in [FontFamily::Proportional, FontFamily::Monospace, bold()] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(1, "noto-sc".into());
    }
    // A system CJK font, if there is one, covers the rarer characters people
    // type into track and clip names.
    if let Some(bytes) = CJK_CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts
            .font_data
            .insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace, bold()] {
            fonts.families.entry(family).or_default().push("cjk".into());
        }
    }
    ctx.set_fonts(fonts);
}

// ---- style --------------------------------------------------------------------

/// Install fonts and the AIDAW style. Call once at start-up.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals(visuals());
    ctx.style_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, bold_font(17.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        let sp = &mut s.spacing;
        sp.item_spacing = egui::vec2(8.0, 6.0);
        sp.button_padding = egui::vec2(10.0, 5.0);
        sp.interact_size.y = 26.0;
        sp.slider_rail_height = 6.0;
        sp.window_margin = Margin::same(16);
        sp.menu_margin = Margin::same(8);
        sp.combo_width = 120.0;
        sp.icon_width = 16.0;
        sp.scroll = egui::style::ScrollStyle::thin();
    });
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let r = CornerRadius::same(7);
    v.panel_fill = PANEL;
    v.window_fill = SURFACE;
    v.extreme_bg_color = CANVAS;
    v.faint_bg_color = SURFACE;
    v.code_bg_color = SURFACE_HI;
    v.window_stroke = Stroke::new(1.0_f32, BORDER_HI);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow {
        offset: [0, 10],
        blur: 36,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };
    v.hyperlink_color = CYAN;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.selection.bg_fill = alpha(ACCENT, 150);
    v.selection.stroke = Stroke::new(1.0_f32, rgb(0xd6, 0xcc, 0xff));

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
    w.noninteractive.corner_radius = r;

    w.inactive.weak_bg_fill = SURFACE_HI;
    w.inactive.bg_fill = SURFACE_HI;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.inactive.corner_radius = r;

    w.hovered.weak_bg_fill = mix(SURFACE_HI, ACCENT, 0.18);
    w.hovered.bg_fill = mix(SURFACE_HI, ACCENT, 0.18);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, alpha(ACCENT, 170));
    w.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 1.0;

    w.active.weak_bg_fill = mix(SURFACE_HI, ACCENT, 0.4);
    w.active.bg_fill = mix(SURFACE_HI, ACCENT, 0.4);
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke = Stroke::new(2.0_f32, Color32::WHITE);
    w.active.corner_radius = r;

    w.open.weak_bg_fill = mix(SURFACE_HI, ACCENT, 0.25);
    w.open.bg_fill = SURFACE_HI;
    w.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.open.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    w.open.corner_radius = r;
    v
}

/// Panel frames: top bar, side bar, editors, status bar.
pub fn chrome_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CHROME)
        .inner_margin(Margin::symmetric(12, 8))
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .inner_margin(Margin::same(10))
}

/// A rounded card on a panel (a track strip, the LCD…).
pub fn card(fill: Color32, stroke: Stroke) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 8))
}

// ---- painting helpers ---------------------------------------------------------

/// A soft coloured halo around `rect`.
pub fn glow(painter: &Painter, rect: Rect, radius: u8, color: Color32, blur: u8) {
    painter.add(
        Shadow {
            offset: [0, 0],
            blur,
            spread: 0,
            color,
        }
        .as_shape(rect, CornerRadius::same(radius)),
    );
}

/// A big, glowing, rounded button with a solid colour, for the few actions
/// that matter most (Play, Ask AI).
pub fn hero_button(
    ui: &mut egui::Ui,
    icon: Option<crate::icons::Icon>,
    text: &str,
    color: Color32,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hot = resp.hovered();
        let down = resp.is_pointer_button_down_on();
        let radius = (size.y / 2.0).min(255.0) as u8;
        glow(
            p,
            rect,
            radius,
            alpha(color, if hot { 120 } else { 70 }),
            18,
        );
        let fill = if down {
            mix(color, Color32::BLACK, 0.15)
        } else if hot {
            mix(color, Color32::WHITE, 0.12)
        } else {
            color
        };
        p.rect_filled(rect, radius, fill);
        // A thin highlight on the upper edge gives it some depth.
        p.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, alpha(Color32::WHITE, 60)),
            egui::StrokeKind::Inside,
        );
        let ink = if color.r() as u32 + color.g() as u32 + color.b() as u32 > 420 {
            CANVAS
        } else {
            Color32::WHITE
        };
        // Icon and text are centred together as one group.
        let galley = p.layout_no_wrap(text.into(), bold_font(15.0), ink);
        let icon_w = if icon.is_some() { 16.0 + 7.0 } else { 0.0 };
        let left = rect.center().x - (icon_w + galley.size().x) / 2.0;
        if let Some(icon) = icon {
            let r = Rect::from_center_size(
                egui::pos2(left + 8.0, rect.center().y),
                egui::vec2(16.0, 16.0),
            );
            crate::icons::paint(ui, r, icon, ink);
        }
        let pos = egui::pos2(left + icon_w, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, ink);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A small rounded tag, e.g. the M/S flags on a track header, placed with
/// its top-right corner at `right_top`.
pub fn pill(painter: &Painter, right_top: egui::Pos2, text: &str, color: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.into(), bold_font(10.0), CANVAS);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = Rect::from_min_size(right_top - egui::vec2(size.x, 0.0), size);
    painter.rect_filled(rect, 6.0, color);
    painter.galley(rect.min + egui::vec2(5.0, 2.0), galley, CANVAS);
    rect
}

/// Upper-case, letter-spaced section title, e.g. "YOUR SONG".
pub fn section(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(bold_font(12.5))
            .extra_letter_spacing(1.2)
            .color(TEXT),
    )
}

/// Dim helper text in a toolbar.
pub fn hint(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(text).size(12.0).color(TEXT_FAINT))
}
