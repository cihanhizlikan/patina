//! ui.rs — Editor panel rendered with egui.
//!
//! Design tokens and layout follow the handoff README §1–§16.
//! All sizing derives from a single U-unit computed from window height.

use crate::editor::{EditParams, Preset};
use egui::{
    Color32, Context, Frame, Margin, Rect, Response, RichText, Rounding, ScrollArea, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use egui_extras;

// ── Colour tokens (§3) ────────────────────────────────────────────────────────

#[allow(dead_code)]
pub const CANVAS: Color32 = Color32::from_rgb(0x0E, 0x0E, 0x10);
pub const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
pub const BG_DEEP: Color32 = Color32::from_rgb(0x16, 0x16, 0x18);
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2E);
#[allow(dead_code)]
pub const BORDER_HI: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3F);
pub const BTN: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2E);
#[allow(dead_code)]
pub const BTN_PRESS: Color32 = Color32::from_rgb(0x3E, 0x3E, 0x44);
pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
pub const DIM: Color32 = Color32::from_rgb(0x8E, 0x8E, 0x93);
pub const FAINT: Color32 = Color32::from_rgb(0x5E, 0x5E, 0x63);
pub const ACCENT: Color32 = Color32::from_rgb(0x9D, 0xBF, 0xB0);
#[allow(dead_code)]
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x7E, 0xA3, 0x95);
// Semi-transparent: accentBg = rgba(157,191,176,0.16), accentBdr = rgba(157,191,176,0.45)
pub const ACCENT_BG: Color32 = Color32::from_rgba_premultiplied(25, 47, 41, 41);
pub const ACCENT_BDR: Color32 = Color32::from_rgba_premultiplied(71, 86, 79, 115);
pub const DANGER: Color32 = Color32::from_rgb(0xE0, 0x65, 0x4A);
pub const DANGER_BG: Color32 = Color32::from_rgb(0x3A, 0x26, 0x24);
pub const DANGER_BDR: Color32 = Color32::from_rgb(0x5A, 0x30, 0x28);
pub const DANGER_PR: Color32 = Color32::from_rgb(0x4A, 0x28, 0x24);
pub const ON_PRIMARY: Color32 = Color32::from_rgb(0x15, 0x20, 0x1B);
#[allow(dead_code)]
pub const TRACK: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x32);

/// Pressed-state accent fill used by the primary button and modal primary hover.
const ACCENT_PRESS: Color32 = Color32::from_rgba_premultiplied(35, 68, 59, 56);
/// Primary hover — slightly lighter than ACCENT so hover reads against the solid fill.
const ACCENT_HOVER: Color32 = Color32::from_rgb(0x8D, 0xAF, 0xA0);

/// Rail width: 20% of window width, clamped to [320, 420] logical px.
pub fn rail_width(window_w: f32) -> f32 {
    (window_w * 0.20).clamp(320.0, 420.0)
}

// ── Design-token struct (U-derived, computed once per frame §1) ───────────────

/// All layout sizes derived from the base unit U = clamp(window_h/60, 13, 22).
struct T {
    u: f32,
    row_x: f32,    // horizontal rail padding
    row_y: f32,    // vertical row padding
    gap: f32,      // standard gap between siblings
    gap_t: f32,    // tight gap (within a button group)
    label_w: f32,  // width of FILE/HISTORY label column
    btn_h: f32,    // button height
    slider_h: f32, // slider row height
    slbl_w: f32,   // slider label column width
    sval_w: f32,   // slider value column width
    radius: f32,   // button/tile corner radius
    dot: f32,      // section dot radius
    icon: f32,     // icon size (square)
    icon_chk: f32, // check icon size inside active badge
    badge: f32,    // preset active-badge diameter
    text_sec: f32, // section header font size
    text_sub: f32, // subgroup heading font size
    text_btn: f32, // button label font size
    text_sld: f32, // slider label/value font size
    rail_w: f32,   // computed rail width this frame
}

impl T {
    fn new(ctx: &Context) -> Self {
        let h = ctx.screen_rect().height();
        let w = ctx.screen_rect().width();
        let u = (h / 60.0).clamp(13.0, 22.0);
        Self {
            u,
            row_x: (u * 1.000).max(14.0),
            row_y: (u * 0.778).max(11.0),
            gap: (u * 0.556).max(8.0),
            gap_t: (u * 0.444).max(6.0),
            label_w: (u * 3.555).max(56.0),
            btn_h: (u * 1.778).max(24.0),
            slider_h: (u * 1.778).max(24.0),
            slbl_w: (u * 4.444).max(64.0),
            sval_w: (u * 2.667).max(40.0),
            radius: (u * 0.333).max(4.0),
            dot: (u * 0.500).max(7.0),
            icon: (u * 0.778).max(12.0),
            icon_chk: (u * 0.611).max(9.0),
            badge: (u * 1.000).max(14.0),
            text_sec: (u * 0.583).max(9.5),
            text_sub: (u * 0.555).max(9.5),
            text_btn: (u * 0.667).max(11.0),
            text_sld: (u * 0.667).max(11.0),
            rail_w: rail_width(w),
        }
    }
}

// ── Preset group order (§7, README) ──────────────────────────────────────────

struct PresetGroup {
    name: &'static str,
    items: &'static [Preset],
}

static PRESET_GROUPS: &[PresetGroup] = &[
    PresetGroup {
        name: "QUICK FIXES",
        items: &[Preset::AutoEnhance, Preset::Vivid, Preset::Punch],
    },
    PresetGroup {
        name: "BRIGHT & AIRY",
        items: &[Preset::CleanBright, Preset::SoftGlow],
    },
    PresetGroup {
        name: "FILM",
        items: &[Preset::WarmFilm, Preset::GoldenHour, Preset::FadedMatte],
    },
    PresetGroup {
        name: "DRAMATIC",
        items: &[Preset::Moody, Preset::Chrome],
    },
    PresetGroup {
        name: "BLACK & WHITE",
        items: &[Preset::Bw, Preset::BwContrast],
    },
];

// ── Icon cache ────────────────────────────────────────────────────────────────

pub struct Icons {
    pub save: TextureHandle,
    pub save_as: TextureHandle,
    pub undo: TextureHandle,
    pub redo: TextureHandle,
    pub reset: TextureHandle,
    pub rotate_left: TextureHandle,
    pub rotate_right: TextureHandle,
    pub flip_h: TextureHandle,
    pub flip_v: TextureHandle,
    pub chevron: TextureHandle,
    pub check: TextureHandle,
}

impl Icons {
    pub fn load(ctx: &Context) -> Self {
        // SVGs use stroke="currentColor" which rasterizes to black.
        // Recolor all non-transparent pixels to white so egui's multiplicative
        // tint works: white × fg_color = fg_color.
        fn load_svg(ctx: &Context, name: &str, bytes: &[u8]) -> TextureHandle {
            let mut img = egui_extras::image::load_svg_bytes(bytes)
                .unwrap_or_else(|_| egui::ColorImage::new([24, 24], Color32::WHITE));
            for px in img.pixels.iter_mut() {
                if px.a() > 0 {
                    *px = Color32::from_rgba_premultiplied(px.a(), px.a(), px.a(), px.a());
                }
            }
            ctx.load_texture(name, img, TextureOptions::LINEAR)
        }
        Self {
            save: load_svg(ctx, "ic_save", include_bytes!("assets/icons/save.svg")),
            save_as: load_svg(
                ctx,
                "ic_save_as",
                include_bytes!("assets/icons/save_as.svg"),
            ),
            undo: load_svg(ctx, "ic_undo", include_bytes!("assets/icons/undo.svg")),
            redo: load_svg(ctx, "ic_redo", include_bytes!("assets/icons/redo.svg")),
            reset: load_svg(ctx, "ic_reset", include_bytes!("assets/icons/reset.svg")),
            rotate_left: load_svg(
                ctx,
                "ic_rot_l",
                include_bytes!("assets/icons/rotate_left.svg"),
            ),
            rotate_right: load_svg(
                ctx,
                "ic_rot_r",
                include_bytes!("assets/icons/rotate_right.svg"),
            ),
            flip_h: load_svg(ctx, "ic_flip_h", include_bytes!("assets/icons/flip_h.svg")),
            flip_v: load_svg(ctx, "ic_flip_v", include_bytes!("assets/icons/flip_v.svg")),
            chevron: load_svg(
                ctx,
                "ic_chevron",
                include_bytes!("assets/icons/chevron_down.svg"),
            ),
            check: load_svg(ctx, "ic_check", include_bytes!("assets/icons/check.svg")),
        }
    }
}

// ── Thumbnail cache ───────────────────────────────────────────────────────────

pub struct ThumbCache {
    handles: Vec<Option<TextureHandle>>,
}

impl Default for ThumbCache {
    fn default() -> Self {
        Self {
            handles: vec![None; Preset::ALL.len()],
        }
    }
}

impl ThumbCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upload any not-yet-uploaded thumbnails. Safe to call every frame.
    pub fn upload(&mut self, ctx: &Context) {
        for (i, slot) in self.handles.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            let bytes = crate::preset_thumbs::thumb_svg_for_index(i);
            let img = match egui_extras::image::load_svg_bytes(bytes) {
                Ok(img) => img,
                Err(_) => egui::ColorImage::new([108, 81], Color32::from_rgb(0x1E, 0x2B, 0x26)),
            };
            *slot = Some(ctx.load_texture(format!("pt_{i}"), img, TextureOptions::LINEAR));
        }
    }

    fn get(&self, i: usize) -> Option<&TextureHandle> {
        self.handles.get(i)?.as_ref()
    }
}

// ── Panel action ──────────────────────────────────────────────────────────────

/// Intentions signalled from the UI to the app in a single frame.
#[derive(Default)]
pub struct PanelAction {
    pub save: bool,
    pub save_as: bool,
    pub undo: bool,
    pub redo: bool,
    pub reset_all: bool,
    pub rot_cw: bool,
    pub rot_ccw: bool,
    pub flip_h: bool,
    pub flip_v: bool,
    pub new_params: Option<EditParams>,
}

// ── Global egui style (called every frame from main.rs) ───────────────────────

pub fn apply_panel_style(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    // Transparent panel_fill so the wgpu image shows through the canvas area.
    style.visuals.panel_fill = Color32::TRANSPARENT;
    style.visuals.window_fill = BG;
    style.visuals.override_text_color = Some(TEXT);
    // Widget colours (apply to egui's own widgets; custom buttons override per-call)
    style.visuals.widgets.inactive.bg_fill = BTN;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = ACCENT_BG;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_BDR);
    style.visuals.widgets.active.bg_fill = ACCENT_PRESS;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.slider_trailing_fill = true;
    style.visuals.selection.bg_fill = ACCENT; // slider filled track
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    // Scrollbar
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.handle_min_length = 24.0;
    style.visuals.extreme_bg_color = Color32::TRANSPARENT;
    ctx.set_style(style);
}

// ── Panel entry point ─────────────────────────────────────────────────────────

pub fn draw_panel(
    ctx: &Context,
    params: &EditParams,
    can_undo: bool,
    can_redo: bool,
    thumbs: &ThumbCache,
    icons: &Icons,
    open_sections: &mut [bool; 6],
    metadata: Option<&crate::exif::ImageMetadata>,
) -> PanelAction {
    let mut action = PanelAction::default();
    let mut p = params.clone();
    let t = T::new(ctx);

    egui::SidePanel::right("editor_rail")
        .resizable(false)
        .exact_width(t.rail_w)
        .frame(Frame::none().fill(BG).stroke(Stroke::new(1.0, BORDER)))
        .show(ctx, |ui| {
            ui.set_width(t.rail_w);

            header_row(ui, &t, "FILE", |ui| {
                if icon_btn_primary(ui, &t, &icons.save, "Save").clicked() {
                    action.save = true;
                }
                if icon_btn(ui, &t, &icons.save_as, "Save As").clicked() {
                    action.save_as = true;
                }
            });

            header_row(ui, &t, "HISTORY", |ui| {
                // When the available width is too narrow for all three labelled buttons,
                // drop the text labels and show icons only. This prevents overlap at any
                // panel width including maximised-window narrow states.
                let avail = ui.available_width();
                let labels = if avail >= 230.0 {
                    ("Undo", "Redo", "Reset")
                } else {
                    ("", "", "")
                };

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_btn_danger(ui, &t, &icons.reset, labels.2).clicked() {
                        action.reset_all = true;
                    }
                    ui.add_enabled_ui(can_redo, |ui| {
                        if icon_btn(ui, &t, &icons.redo, labels.1).clicked() {
                            action.redo = true;
                        }
                    });
                    ui.add_enabled_ui(can_undo, |ui| {
                        if icon_btn(ui, &t, &icons.undo, labels.0).clicked() {
                            action.undo = true;
                        }
                    });
                });
            });

            hairline(ui);

            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .drag_to_scroll(false)
                .show(ui, |ui| {
                    ui.set_width(t.rail_w);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;

                    let mut params_changed = false;

                    if section(ui, &t, icons, "PRESETS", &mut open_sections[0]) {
                        if let Some(new_preset) = draw_presets(ui, &t, params, thumbs, icons) {
                            p.preset = new_preset;
                            params_changed = true;
                        }
                    }

                    if section(ui, &t, icons, "TRANSFORM", &mut open_sections[1]) {
                        // draw_transform returns true when the scale slider changed;
                        // rotation/flip set action flags directly and are handled separately.
                        if draw_transform(ui, &t, icons, &mut p, &mut action) {
                            params_changed = true;
                        }
                    }

                    if section(ui, &t, icons, "EXIF / IPTC", &mut open_sections[2]) {
                        draw_exif_section(ui, &t, metadata);
                    }

                    if section(ui, &t, icons, "TONE", &mut open_sections[3]) {
                        params_changed |= draw_sliders(
                            ui,
                            &t,
                            &mut [
                                SD::bipolar("Exposure", &mut p.exposure, -2.0..=2.0),
                                SD::bipolar("Highlights", &mut p.highlights, -1.0..=1.0),
                                SD::bipolar("Shadows", &mut p.shadows, -1.0..=1.0),
                                SD::bipolar("Brightness", &mut p.brightness, -1.0..=1.0),
                                SD::bipolar("Contrast", &mut p.contrast, -1.0..=1.0),
                            ],
                        );
                    }

                    if section(ui, &t, icons, "COLOR", &mut open_sections[4]) {
                        params_changed |= draw_sliders(
                            ui,
                            &t,
                            &mut [
                                SD::bipolar("Saturation", &mut p.saturation, -1.0..=1.0),
                                SD::bipolar("Vibrance", &mut p.vibrance, -1.0..=1.0),
                            ],
                        );
                    }

                    if section(ui, &t, icons, "DETAIL", &mut open_sections[5]) {
                        params_changed |= draw_sliders(
                            ui,
                            &t,
                            &mut [SD::unipolar("Sharpen", &mut p.sharpen, 0.0..=1.0)],
                        );
                    }

                    // Single clone: only after all sections have had a chance to mutate p.
                    if params_changed {
                        action.new_params = Some(p.clone());
                    }
                });
        });

    action
}

// ── Button appearance ─────────────────────────────────────────────────────────

/// The visual state of a button, derived from its egui response.
#[derive(Clone, Copy)]
enum BtnState {
    Rest,
    Hover,
    Press,
    Disabled,
}

impl BtnState {
    fn from_resp(resp: &Response, ui: &Ui) -> Self {
        if !ui.is_enabled() {
            return Self::Disabled;
        }
        if resp.is_pointer_button_down_on() {
            return Self::Press;
        }
        if resp.hovered() {
            return Self::Hover;
        }
        Self::Rest
    }
}

/// Colour triple (bg, fg, border) for a standard dark button.
fn default_btn_colors(state: BtnState) -> (Color32, Color32, Color32) {
    match state {
        BtnState::Rest => (BTN, TEXT, BORDER),
        BtnState::Hover => (ACCENT_BG, ACCENT, ACCENT_BDR),
        BtnState::Press => (ACCENT_PRESS, ACCENT, ACCENT_BDR),
        BtnState::Disabled => (BTN, FAINT, BORDER),
    }
}

/// Colour triple for the danger (Reset/Delete) button variant.
fn danger_btn_colors(state: BtnState) -> (Color32, Color32, Color32) {
    match state {
        BtnState::Rest => (BTN, TEXT, BORDER),
        BtnState::Hover => (DANGER_BG, DANGER, DANGER_BDR),
        BtnState::Press => (DANGER_PR, DANGER, DANGER_BDR),
        BtnState::Disabled => (BTN, FAINT, BORDER),
    }
}

// ── Button painter (shared core) ──────────────────────────────────────────────

/// Geometry spec for a button's icon + label layout.
struct BtnLayout {
    rect: Rect,
    /// X position where icon starts.
    icon_x: f32,
    /// X position where text starts (after icon + gap, or = icon_x if no icon).
    text_x: f32,
    icon_sz: f32,
}

/// Allocate a button rect and compute icon/text positions.
/// `width = None` → content-fitted width; `Some(w)` → exact width (centred content).
fn alloc_btn(
    ui: &mut Ui,
    label: &str,
    has_icon: bool,
    font_sz: f32,
    height: f32,
    width: Option<f32>,
    icon_sz: f32,
    pad: f32,
    gap: f32,
) -> (BtnLayout, Response) {
    let lbl_w = if label.is_empty() {
        0.0
    } else {
        ui.fonts(|f| {
            f.layout_no_wrap(label.to_string(), egui::FontId::proportional(font_sz), TEXT)
                .size()
                .x
        })
    };
    let ic_w = if has_icon {
        icon_sz + if label.is_empty() { 0.0 } else { gap }
    } else {
        0.0
    };
    let content_w = ic_w + lbl_w;
    // Icon-only buttons use symmetric padding; labelled buttons use full pad on both sides.
    let effective_pad = if label.is_empty() && has_icon {
        pad * 0.75
    } else {
        pad
    };
    let btn_w = width.unwrap_or(effective_pad + content_w + effective_pad);

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(btn_w, height), Sense::click());

    let start_x = if width.is_some() {
        // Centre content within fixed-width button
        rect.center().x - content_w * 0.5
    } else {
        rect.min.x + pad
    };

    let layout = BtnLayout {
        rect,
        icon_x: start_x,
        text_x: start_x + ic_w,
        icon_sz,
    };
    (layout, resp)
}

/// Paint a button given its layout, colours, optional icon, and label.
fn paint_btn(
    ui: &Ui,
    layout: &BtnLayout,
    radius: f32,
    bg: Color32,
    fg: Color32,
    border: Option<(f32, Color32)>,
    icon: Option<&TextureHandle>,
    label: &str,
    font_sz: f32,
) {
    let p = ui.painter();
    let r = Rounding::same(radius);
    p.rect_filled(layout.rect, r, bg);
    if let Some((w, c)) = border {
        p.rect_stroke(layout.rect, r, Stroke::new(w, c));
    }
    if let Some(tex) = icon {
        let ic_rect = Rect::from_min_size(
            egui::pos2(layout.icon_x, layout.rect.center().y - layout.icon_sz * 0.5),
            Vec2::splat(layout.icon_sz),
        );
        p.image(
            tex.id(),
            ic_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            fg,
        );
    }
    p.text(
        egui::pos2(layout.text_x, layout.rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(font_sz),
        fg,
    );
}

// ── Named button variants (thin wrappers over the shared core) ────────────────

/// Standard dark button with icon — used for Save As, Undo, Redo.
fn icon_btn(ui: &mut Ui, t: &T, icon: &TextureHandle, label: &str) -> Response {
    let (layout, resp) = alloc_btn(
        ui,
        label,
        true,
        t.text_btn,
        t.btn_h,
        None,
        t.icon,
        t.gap,
        (t.u * 0.39).max(5.0),
    );
    let (bg, fg, bdr) = default_btn_colors(BtnState::from_resp(&resp, ui));
    paint_btn(
        ui,
        &layout,
        t.radius,
        bg,
        fg,
        Some((1.0, bdr)),
        Some(icon),
        label,
        t.text_btn,
    );
    resp
}

/// Solid-accent primary button — Save only.
fn icon_btn_primary(ui: &mut Ui, t: &T, icon: &TextureHandle, label: &str) -> Response {
    let (layout, resp) = alloc_btn(
        ui,
        label,
        true,
        t.text_btn,
        t.btn_h,
        None,
        t.icon,
        t.gap,
        (t.u * 0.39).max(5.0),
    );
    let bg = if resp.is_pointer_button_down_on() {
        ACCENT_DIM
    } else if resp.hovered() {
        ACCENT_HOVER
    } else {
        ACCENT
    };
    paint_btn(
        ui,
        &layout,
        t.radius,
        bg,
        ON_PRIMARY,
        None,
        Some(icon),
        label,
        t.text_btn,
    );
    resp
}

/// Danger-tint button — Reset only.
fn icon_btn_danger(ui: &mut Ui, t: &T, icon: &TextureHandle, label: &str) -> Response {
    let (layout, resp) = alloc_btn(
        ui,
        label,
        true,
        t.text_btn,
        t.btn_h,
        None,
        t.icon,
        t.gap,
        (t.u * 0.39).max(5.0),
    );
    let (bg, fg, bdr) = danger_btn_colors(BtnState::from_resp(&resp, ui));
    paint_btn(
        ui,
        &layout,
        t.radius,
        bg,
        fg,
        Some((1.0, bdr)),
        Some(icon),
        label,
        t.text_btn,
    );
    resp
}

/// Full-width momentary button for the transform grid (Rotate left/right).
fn icon_btn_full(ui: &mut Ui, t: &T, icon: &TextureHandle, label: &str, width: f32) -> Response {
    let (layout, resp) = alloc_btn(
        ui,
        label,
        true,
        t.text_btn,
        t.btn_h,
        Some(width),
        t.icon,
        t.gap,
        (t.u * 0.39).max(5.0),
    );
    // Rotate buttons: hover tints but never persist after release.
    let (bg, fg, bdr) = default_btn_colors(if resp.hovered() {
        BtnState::Hover
    } else {
        BtnState::Rest
    });
    paint_btn(
        ui,
        &layout,
        t.radius,
        bg,
        fg,
        Some((1.0, bdr)),
        Some(icon),
        label,
        t.text_btn,
    );
    resp
}

/// Full-width toggle button for the transform grid (Flip H/V).
fn icon_btn_toggle_full(
    ui: &mut Ui,
    t: &T,
    icon: &TextureHandle,
    label: &str,
    active: bool,
    width: f32,
) -> Response {
    let (layout, resp) = alloc_btn(
        ui,
        label,
        true,
        t.text_btn,
        t.btn_h,
        Some(width),
        t.icon,
        t.gap,
        (t.u * 0.39).max(5.0),
    );
    let (bg, fg, bdr, bdr_w) = if active {
        (ACCENT_BG, ACCENT, ACCENT, 1.0f32)
    } else if resp.hovered() {
        (ACCENT_BG, ACCENT, ACCENT_BDR, 1.0)
    } else {
        (BTN, TEXT, BORDER, 1.0)
    };
    paint_btn(
        ui,
        &layout,
        t.radius,
        bg,
        fg,
        Some((bdr_w, bdr)),
        Some(icon),
        label,
        t.text_btn,
    );
    resp
}

// ── Structural layout helpers ─────────────────────────────────────────────────

/// FILE / HISTORY rows: faint dot + fixed label + inline buttons, same height as sections.
fn header_row(ui: &mut Ui, t: &T, label: &str, add_buttons: impl FnOnce(&mut Ui)) {
    let row_h = t.btn_h + t.row_y * 2.0;
    let avail_w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, row_h), Sense::hover());
    let cy = rect.center().y;

    let dot_cx = rect.min.x + t.row_x + t.dot;
    ui.painter()
        .circle_filled(egui::pos2(dot_cx, cy), t.dot * 0.5, FAINT);
    ui.painter().text(
        egui::pos2(dot_cx + t.dot + t.gap_t, cy),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(t.text_sec),
        TEXT,
    );

    let btn_x = rect.min.x + t.row_x + t.dot * 2.0 + t.gap_t + t.label_w + t.gap_t;
    let btn_rect = Rect::from_min_max(
        egui::pos2(btn_x, rect.min.y),
        egui::pos2(rect.max.x - t.row_x, rect.max.y),
    );
    ui.allocate_ui_at_rect(btn_rect, |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = t.gap_t;
            add_buttons(ui);
        });
    });
}

/// Collapsible section header with spruce dot, title, chevron. Returns `true` when open.
fn section(ui: &mut Ui, t: &T, icons: &Icons, title: &str, open: &mut bool) -> bool {
    let row_h = t.btn_h + t.row_y * 2.0;
    let avail_w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail_w, row_h), Sense::click());
    if resp.clicked() {
        *open = !*open;
    }

    let pt = ui.painter();
    let cy = rect.center().y;

    if resp.hovered() {
        pt.rect_filled(rect, Rounding::ZERO, Color32::from_white_alpha(5));
    }

    // Spruce dot with halo
    let cx = rect.min.x + t.row_x + t.dot;
    pt.circle_filled(
        egui::pos2(cx, cy),
        t.dot * 0.83,
        Color32::from_rgba_premultiplied(25, 47, 41, 41),
    );
    pt.circle_filled(egui::pos2(cx, cy), t.dot * 0.50, ACCENT);

    pt.text(
        egui::pos2(cx + t.dot * 1.5 + t.gap_t * 0.5, cy),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(t.text_sec),
        TEXT,
    );

    // Chevron: image when open (▾), line segments when closed (›)
    let ic_sz = Vec2::splat(t.icon);
    let ic_rect = Rect::from_min_size(
        egui::pos2(rect.max.x - t.row_x - t.icon, cy - t.icon * 0.5),
        ic_sz,
    );

    if *open {
        pt.image(
            icons.chevron.id(),
            ic_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            DIM,
        );
    } else {
        let xi = ic_rect.min.x + ic_sz.x * 0.3;
        let xo = ic_rect.max.x - ic_sz.x * 0.3;
        let mk = Stroke::new(1.5, DIM);
        pt.line_segment(
            [
                egui::pos2(xi, ic_rect.min.y + ic_sz.y * 0.2),
                egui::pos2(xo, ic_rect.center().y),
            ],
            mk,
        );
        pt.line_segment(
            [
                egui::pos2(xo, ic_rect.center().y),
                egui::pos2(xi, ic_rect.max.y - ic_sz.y * 0.2),
            ],
            mk,
        );
    }

    pt.hline(rect.x_range(), rect.min.y, Stroke::new(1.0, BORDER));
    *open
}

fn hairline(ui: &mut Ui) {
    let r = ui.available_rect_before_wrap();
    ui.painter()
        .hline(r.x_range(), r.top(), Stroke::new(1.0, BORDER));
    ui.add_space(1.0);
}

// ── Preset grid ───────────────────────────────────────────────────────────────

fn draw_presets(
    ui: &mut Ui,
    t: &T,
    params: &EditParams,
    thumbs: &ThumbCache,
    icons: &Icons,
) -> Option<Preset> {
    let mut selected = None;
    let cols = 3usize;
    let spacing = t.gap;
    let tile_w =
        ((t.rail_w - t.row_x * 2.0 - spacing * (cols as f32 - 1.0)) / cols as f32).max(88.0);
    let thumb_h = tile_w * 0.75;
    let lbl_h = t.row_y * 0.5 + t.text_btn + t.row_y * 0.5;
    let tile_h = thumb_h + lbl_h;

    for (g_idx, grp) in PRESET_GROUPS.iter().enumerate() {
        ui.add_space(if g_idx == 0 {
            t.row_y * 0.5
        } else {
            t.u * 0.78
        });
        Frame::none()
            .inner_margin(Margin {
                left: t.row_x,
                ..Margin::ZERO
            })
            .show(ui, |ui| {
                ui.label(
                    RichText::new(grp.name)
                        .color(FAINT)
                        .size(t.text_sub)
                        .strong(),
                );
            });
        ui.add_space(t.gap_t);

        for chunk in grp.items.chunks(cols) {
            let row_top = ui.cursor().top();
            let row_left = ui.cursor().left() + t.row_x;
            // §0: allocate_rect advances the cursor — no add_space after
            ui.allocate_rect(
                Rect::from_min_size(
                    egui::pos2(ui.cursor().left(), row_top),
                    Vec2::new(t.rail_w, tile_h),
                ),
                Sense::hover(),
            );

            for (col_i, &preset) in chunk.iter().enumerate() {
                let idx = Preset::ALL.iter().position(|&p| p == preset).unwrap_or(0);
                let active = params.preset == preset;
                let tile_x = row_left + col_i as f32 * (tile_w + spacing);
                let tile =
                    Rect::from_min_size(egui::pos2(tile_x, row_top), Vec2::new(tile_w, tile_h));
                let resp =
                    ui.interact(tile, egui::Id::new(("pt", preset as usize)), Sense::click());
                let p = ui.painter();

                // Tile background: subtle hover tint (same toggle logic as flip buttons)
                let tile_bg = if resp.hovered() && !active {
                    Color32::from_rgba_premultiplied(20, 38, 34, 28)
                } else {
                    BG_DEEP
                };
                p.rect_filled(tile, Rounding::same(t.radius), tile_bg);

                // Thumbnail image
                let thumb = Rect::from_min_size(tile.min, Vec2::new(tile_w, thumb_h));
                if let Some(tex) = thumbs.get(idx) {
                    p.image(
                        tex.id(),
                        thumb,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                // Active badge: spruce circle with check icon
                if active {
                    let badge_r = t.badge * 0.5;
                    let off = t.u * 0.33;
                    let bc = egui::pos2(thumb.max.x - off - badge_r, thumb.min.y + off + badge_r);
                    p.circle_filled(bc, badge_r, ACCENT);
                    p.image(
                        icons.check.id(),
                        Rect::from_center_size(bc, Vec2::splat(t.icon_chk)),
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        ON_PRIMARY,
                    );
                }

                // Label strip
                let lbl_rect = Rect::from_min_size(
                    egui::pos2(tile.min.x, tile.min.y + thumb_h),
                    Vec2::new(tile_w, lbl_h),
                );
                p.rect_filled(
                    lbl_rect,
                    Rounding {
                        nw: 0.0,
                        ne: 0.0,
                        sw: t.radius,
                        se: t.radius,
                    },
                    BG_DEEP,
                );
                p.text(
                    lbl_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    preset.label(),
                    egui::FontId::proportional(t.text_sld),
                    if active || resp.hovered() {
                        ACCENT
                    } else {
                        TEXT
                    },
                );

                // Border: solid accent when active, spruce when hovered, dim when rest
                let (sc, sw) = if active {
                    (ACCENT, 1.5)
                } else if resp.hovered() {
                    (ACCENT_BDR, 1.0)
                } else {
                    (BORDER, 1.0)
                };
                p.rect_stroke(tile, Rounding::same(t.radius), Stroke::new(sw, sc));

                if resp.clicked() {
                    // Click active preset → deselect; click inactive → select
                    selected = Some(if active { Preset::None } else { preset });
                }
            }
        }
    }
    ui.add_space(t.row_y);
    selected
}

// ── Transform section ─────────────────────────────────────────────────────────

fn draw_transform(
    ui: &mut Ui,
    t: &T,
    icons: &Icons,
    p: &mut EditParams,
    action: &mut PanelAction,
) -> bool {
    let btn_w = (t.rail_w - t.row_x * 2.0 - t.gap) / 2.0;

    Frame::none()
        .inner_margin(Margin {
            left: t.row_x,
            right: t.row_x,
            top: t.row_y * 0.5,
            bottom: 0.0,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(t.gap, t.gap);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = t.gap;
                if icon_btn_full(ui, t, &icons.rotate_left, "Rotate left", btn_w).clicked() {
                    action.rot_ccw = true;
                }
                if icon_btn_full(ui, t, &icons.rotate_right, "Rotate right", btn_w).clicked() {
                    action.rot_cw = true;
                }
            });
            ui.add_space(t.gap);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = t.gap;
                if icon_btn_toggle_full(ui, t, &icons.flip_h, "Flip H", p.flip_h, btn_w).clicked() {
                    action.flip_h = true;
                }
                if icon_btn_toggle_full(ui, t, &icons.flip_v, "Flip V", p.flip_v, btn_w).clicked() {
                    action.flip_v = true;
                }
            });
            ui.add_space(t.gap_t);
        });

    scale_slider(ui, t, p)
}

fn scale_slider(ui: &mut Ui, t: &T, p: &mut EditParams) -> bool {
    // Map the nonlinear scale [0.1–1.0–4.0] onto a uniform [0.0–0.5–1.0] slider.
    let to_slider = |s: f32| {
        if s <= 1.0 {
            (s - 0.1) / 0.9 * 0.5
        } else {
            0.5 + (s - 1.0) / 3.0 * 0.5
        }
    };
    let to_scale = |v: f32| {
        if v <= 0.5 {
            0.1 + (v / 0.5) * 0.9
        } else {
            1.0 + ((v - 0.5) / 0.5) * 3.0
        }
    };

    let mut sv = to_slider(p.scale);
    let sv_before = sv;

    Frame::none()
        .inner_margin(Margin {
            left: t.row_x,
            right: t.row_x,
            top: 0.0,
            bottom: 0.0,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (_, lr) = ui.allocate_space(Vec2::new(t.slbl_w, t.slider_h));
                ui.painter().text(
                    lr.left_center(),
                    egui::Align2::LEFT_CENTER,
                    "Scale",
                    egui::FontId::proportional(t.text_sld),
                    DIM,
                );
                ui.add(
                    egui::Slider::new(&mut sv, 0.0..=1.0)
                        .show_value(false)
                        .clamp_to_range(true),
                );
                let (_, vr) = ui.allocate_space(Vec2::new(t.sval_w, t.slider_h));
                ui.painter().text(
                    vr.right_center(),
                    egui::Align2::RIGHT_CENTER,
                    format!("{:.2}", p.scale),
                    egui::FontId::proportional(t.text_sld),
                    TEXT,
                );
            });
        });

    if (sv - sv_before).abs() > 1e-5 {
        p.scale = to_scale(sv);
        true
    } else {
        false
    }
}

// ── Slider section ────────────────────────────────────────────────────────────

/// Slider definition.
struct SD<'a> {
    label: &'static str,
    value: &'a mut f32,
    range: std::ops::RangeInclusive<f32>,
    bipolar: bool,
}

impl<'a> SD<'a> {
    fn bipolar(
        label: &'static str,
        value: &'a mut f32,
        range: std::ops::RangeInclusive<f32>,
    ) -> Self {
        Self {
            label,
            value,
            range,
            bipolar: true,
        }
    }
    fn unipolar(
        label: &'static str,
        value: &'a mut f32,
        range: std::ops::RangeInclusive<f32>,
    ) -> Self {
        Self {
            label,
            value,
            range,
            bipolar: false,
        }
    }
}

fn draw_sliders(ui: &mut Ui, t: &T, defs: &mut [SD<'_>]) -> bool {
    let mut changed = false;
    for def in defs.iter_mut() {
        let before = *def.value;
        Frame::none()
            .inner_margin(Margin {
                left: t.row_x,
                right: t.row_x,
                top: 2.0,
                bottom: 2.0,
            })
            .show(ui, |ui| {
                ui.set_min_height(t.slider_h);
                ui.horizontal(|ui| {
                    let (_, lr) = ui.allocate_space(Vec2::new(t.slbl_w, t.slider_h));
                    ui.painter().text(
                        lr.left_center(),
                        egui::Align2::LEFT_CENTER,
                        def.label,
                        egui::FontId::proportional(t.text_sld),
                        DIM,
                    );

                    let slider = egui::Slider::new(def.value, def.range.clone())
                        .show_value(false)
                        .clamp_to_range(true);
                    let sr = ui.add(slider);

                    // Double-click resets to default value.
                    // We can't use sr.double_clicked() — egui Slider consumes
                    // click events internally. Instead detect via raw pointer state.
                    let double_clicked = ui.input(|i| {
                        i.pointer
                            .button_double_clicked(egui::PointerButton::Primary)
                            && i.pointer
                                .hover_pos()
                                .map(|p| sr.rect.contains(p))
                                .unwrap_or(false)
                    });
                    if double_clicked {
                        *def.value = if def.bipolar { 0.0 } else { *def.range.start() };
                    }

                    let display = format_slider_value(*def.value, def.bipolar);
                    let (_, vr) = ui.allocate_space(Vec2::new(t.sval_w, t.slider_h));
                    ui.painter().text(
                        vr.right_center(),
                        egui::Align2::RIGHT_CENTER,
                        display,
                        egui::FontId::proportional(t.text_sld - 0.5),
                        TEXT,
                    );
                });
            });
        if (*def.value - before).abs() > 1e-5 {
            changed = true;
        }
    }
    changed
}

/// Format a slider value for display. Bipolar uses +/− prefix, 0 → "0".
fn format_slider_value(value: f32, bipolar: bool) -> String {
    if !bipolar {
        return format!("{:.2}", value);
    }
    if value.abs() < 0.005 {
        return "0".into();
    }
    if value > 0.0 {
        format!("+{:.2}", value)
    } else {
        format!("\u{2212}{:.2}", value.abs())
    } // Unicode minus
}

// ── Footer hint ───────────────────────────────────────────────────────────────

// ── EXIF / IPTC panel ─────────────────────────────────────────────────────────

fn draw_exif_section(ui: &mut Ui, t: &T, metadata: Option<&crate::exif::ImageMetadata>) {
    Frame::none()
        .inner_margin(Margin {
            left: t.row_x,
            right: t.row_x,
            top: t.row_y * 0.5,
            bottom: t.row_y,
        })
        .show(ui, |ui| {
            match metadata {
                None => {
                    ui.label(
                        RichText::new("No EXIF data available")
                            .color(FAINT)
                            .size(t.text_sld),
                    );
                }
                Some(meta) if meta.is_empty() => {
                    ui.label(
                        RichText::new("No EXIF data available")
                            .color(FAINT)
                            .size(t.text_sld),
                    );
                }
                Some(meta) => {
                    ui.spacing_mut().item_spacing = egui::Vec2::new(0.0, 5.0);
                    for field in &meta.fields {
                        ui.horizontal(|ui| {
                            // Label column: fixed width, dimmed
                            let (_, lr) = ui.allocate_space(egui::Vec2::new(
                                t.slbl_w + t.sval_w * 0.5,
                                t.text_sld * 1.6,
                            ));
                            ui.painter().text(
                                lr.left_center(),
                                egui::Align2::LEFT_CENTER,
                                field.label,
                                egui::FontId::proportional(t.text_sld - 0.5),
                                FAINT,
                            );
                            // Value: wrapping, full brightness
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(&field.value).color(TEXT).size(t.text_sld),
                                    );
                                },
                            );
                        });
                    }
                }
            }
        });
}
