//! ui.rs — Editor panel rendered with egui.
//!
//! Design tokens and layout follow the handoff README §1–§16.
//! All sizing derives from a single U-unit computed from window height.

use crate::editor::{EditParams, Histogram, Preset};
use egui::{
    Color32, Context, Frame, Margin, Rect, Response, RichText, Rounding, ScrollArea, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use egui_extras;

// ── Colour tokens ─────────────────────────────────────────────────────────────

/// Every colour Patina paints, and the only place one is written. Neutrals
/// carry a trace of the green so the panels sit with the accent instead of
/// next to it. Green *means* something — changed, the default action,
/// selected, focused — and is never decoration. Every text colour reaches
/// WCAG AA (4.5:1) on every surface it is drawn on; `tests` measures it.
/// Opaque except the three drawn over the photo: a translucent colour's
/// contrast depends on what is under it.
pub mod tok {
    use egui::Color32;

    const fn hex(v: u32) -> Color32 {
        Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Behind the image.
    pub const CANVAS: Color32 = hex(0x0D0F0E);
    /// Text fields, slider tracks.
    pub const SURFACE_SUNKEN: Color32 = hex(0x141716);
    /// The rail and dialogs.
    pub const SURFACE: Color32 = hex(0x1A1D1C);
    /// Secondary buttons, chips.
    pub const RAISED: Color32 = hex(0x232826);
    pub const RAISED_HOVER: Color32 = hex(0x2B312E);
    /// Hairlines.
    pub const BORDER: Color32 = hex(0x2A302D);
    /// Outlines, disabled glyphs.
    pub const BORDER_STRONG: Color32 = hex(0x3B423F);

    pub const TEXT_1: Color32 = hex(0xE6EAE8);
    pub const TEXT_2: Color32 = hex(0x9AA39F);
    pub const TEXT_3: Color32 = hex(0x8A938F);
    /// A control that cannot be used now: visible (about 3:1), clearly off.
    pub const TEXT_DISABLED: Color32 = hex(0x5E6662);

    /// The primary button, modified dots, slider fill.
    pub const ACCENT: Color32 = hex(0x9DBFB0);
    pub const ACCENT_HOVER: Color32 = hex(0xB2D3C4);
    pub const ACCENT_PRESS: Color32 = hex(0x87A99A);
    /// Selected fill.
    pub const ACCENT_SOFT: Color32 = hex(0x1F2E28);
    /// Focus ring, selected outline.
    pub const ACCENT_LINE: Color32 = hex(0x4F6E61);
    /// Text on ACCENT.
    pub const ON_ACCENT: Color32 = hex(0x12201A);

    /// Destructive actions, errors.
    pub const DANGER: Color32 = hex(0xE67458);
    pub const DANGER_SOFT: Color32 = hex(0x35221E);
    pub const DANGER_LINE: Color32 = hex(0x5A3028);

    pub const STAR: Color32 = hex(0xE2C062);
    pub const LABEL_RED: Color32 = hex(0xD7604F);
    pub const LABEL_YELLOW: Color32 = hex(0xD9B95A);
    pub const LABEL_GREEN: Color32 = hex(0x6DB487);
    pub const LABEL_BLUE: Color32 = hex(0x6A98CF);
    pub const LABEL_PURPLE: Color32 = hex(0xA178C8);

    // Drawn over the photo, so translucent by design; no text sits on them.
    /// Dims the app behind a dialog.
    pub const SCRIM: Color32 = Color32::from_black_alpha(160);
    /// Composition guide lines.
    pub const GUIDE: Color32 = Color32::from_rgba_premultiplied(60, 60, 60, 60);
    /// The outline that keeps a label dot visible on any photo.
    pub const ON_PHOTO_RING: Color32 = Color32::from_rgba_premultiplied(80, 80, 80, 80);

    /// Histogram channels. Premultiplied with zero alpha, egui adds them, so
    /// where all three overlap reads a quiet grey, not a muddy brown.
    pub const HIST_R: Color32 = Color32::from_rgba_premultiplied(96, 30, 30, 0);
    pub const HIST_G: Color32 = Color32::from_rgba_premultiplied(30, 96, 30, 0);
    pub const HIST_B: Color32 = Color32::from_rgba_premultiplied(30, 30, 96, 0);

    #[cfg(test)]
    mod tests {
        use super::*;

        /// WCAG 2 contrast ratio of two opaque colours.
        fn contrast(a: Color32, b: Color32) -> f32 {
            fn lum(c: Color32) -> f32 {
                let ch = |v: u8| {
                    let v = v as f32 / 255.0;
                    if v <= 0.03928 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
            }
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        }

        #[test]
        fn text_reaches_aa_wherever_it_is_drawn() {
            let pairs = [
                (
                    "TEXT_1",
                    TEXT_1,
                    &[SURFACE, SURFACE_SUNKEN, RAISED, RAISED_HOVER][..],
                ),
                ("TEXT_2", TEXT_2, &[SURFACE, SURFACE_SUNKEN, RAISED][..]),
                ("TEXT_3", TEXT_3, &[SURFACE, SURFACE_SUNKEN][..]),
                (
                    "ON_ACCENT",
                    ON_ACCENT,
                    &[ACCENT, ACCENT_HOVER, ACCENT_PRESS][..],
                ),
                ("ACCENT", ACCENT, &[SURFACE, ACCENT_SOFT, RAISED][..]),
                ("DANGER", DANGER, &[SURFACE, DANGER_SOFT, RAISED][..]),
            ];
            let mut fails = Vec::new();
            for (name, fg, bgs) in pairs {
                for &bg in bgs {
                    let c = contrast(fg, bg);
                    if c < 4.5 {
                        fails.push(format!("{name} on {bg:?}: {c:.2}"));
                    }
                }
            }
            assert!(fails.is_empty(), "{fails:#?}");
        }

        /// Disabled is exempt from AA, but must still be seen.
        #[test]
        fn disabled_is_visible_and_quieter_than_text() {
            let c = contrast(TEXT_DISABLED, SURFACE);
            assert!((2.8..4.5).contains(&c), "{c:.2}");
        }

        /// The token the old palette failed with: `#5E5E63` faint text.
        #[test]
        fn the_measure_agrees_with_the_review() {
            let old_faint = hex(0x5E5E63);
            let old_bg = hex(0x1C1C1E);
            assert!((contrast(old_faint, old_bg) - 2.6).abs() < 0.1);
            assert!((contrast(TEXT_1, SURFACE) - 14.0).abs() < 0.3);
        }
    }
}

use tok::*;

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
    btn_h: f32,    // button height
    slider_h: f32, // slider row height
    slbl_w: f32,   // slider label column width
    sval_w: f32,   // slider value column width
    radius: f32,   // button/tile corner radius
    dot: f32,      // section dot radius
    icon: f32,     // icon size (square)
    caption: f32,  // section heads and sub-labels: the small type step
    body: f32,     // buttons, slider labels and values
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
            btn_h: (u * 1.778).max(24.0),
            slider_h: (u * 1.778).max(24.0),
            slbl_w: (u * 4.444).max(64.0),
            sval_w: (u * 2.667).max(40.0),
            radius: (u * 0.333).max(4.0),
            dot: (u * 0.500).max(7.0),
            icon: (u * 0.778).max(12.0),
            caption: (u * 0.583).max(CAPTION - 1.5),
            body: (u * 0.667).max(CAPTION),
            rail_w: rail_width(w),
        }
    }

    fn btn(&self) -> BtnSize {
        BtnSize {
            h: self.btn_h,
            font: self.body,
            icon: self.icon,
            pad: self.gap,
            gap: (self.u * 0.39).max(5.0),
            radius: self.radius,
            min_w: 0.0,
        }
    }
}

// ── Type ──────────────────────────────────────────────────────────────────────
//
// Three steps, in Inter (bundled): the rail scales its two with the window
// (`T::caption`, `T::body`); dialogs use these. Numbers that change while you
// watch them — slider values — are monospace so they do not jitter.

pub const CAPTION: f32 = 11.0;
pub const BODY: f32 = 13.0;
pub const TITLE: f32 = 16.0;

/// The family for titles: Inter SemiBold. egui has no font weights, so the
/// weight is a family of its own.
pub fn title_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("title".into()))
}

/// Inter for all text, SemiBold for titles, egui's own fonts behind them
/// for glyphs Inter lacks.
pub fn install_fonts(ctx: &Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        FontData::from_static(include_bytes!("assets/fonts/Inter-Regular.ttf")),
    );
    fonts.font_data.insert(
        "Inter SemiBold".into(),
        FontData::from_static(include_bytes!("assets/fonts/Inter-SemiBold.ttf")),
    );
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .expect("egui defines Proportional")
        .insert(0, "Inter".into());
    let mut title = vec!["Inter SemiBold".to_string()];
    title.extend(fallbacks);
    fonts
        .families
        .insert(FontFamily::Name("title".into()), title);
    ctx.set_fonts(fonts);
}

// ── Icon cache ────────────────────────────────────────────────────────────────

/// The pixel size rail icons are drawn at now. Rasterising the SVGs at their
/// own 24 px and scaling to 12–17 px blurred them. It changes with the
/// window height and the display scale, and the cache follows it.
pub fn icon_px(ctx: &Context) -> u32 {
    (T::new(ctx).icon * ctx.pixels_per_point()).round() as u32
}

pub struct Icons {
    /// The pixel size they were rasterised at; see `icon_px`.
    pub px: u32,
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
}

impl Icons {
    pub fn load(ctx: &Context, px: u32) -> Self {
        // SVGs use stroke="currentColor" which rasterizes to black.
        // Recolor all non-transparent pixels to white so egui's multiplicative
        // tint works: white × fg_color = fg_color.
        let load_svg = |ctx: &Context, name: &str, bytes: &[u8]| -> TextureHandle {
            let size = egui::SizeHint::Size(px, px);
            let mut img = egui_extras::image::load_svg_bytes_with_size(bytes, Some(size))
                .unwrap_or_else(|_| egui::ColorImage::new([px as usize; 2], Color32::WHITE));
            for px in img.pixels.iter_mut() {
                if px.a() > 0 {
                    *px = Color32::from_rgba_premultiplied(px.a(), px.a(), px.a(), px.a());
                }
            }
            ctx.load_texture(name, img, TextureOptions::LINEAR)
        };
        Self {
            px,
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
        }
    }
}

// ── Panel action ──────────────────────────────────────────────────────────────

/// What the rail asks the app to do this frame.
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
    /// A change that is a new undo step.
    pub new_params: Option<EditParams>,
    /// A change that continues the slider drag whose first change was the
    /// current undo step: one drag is one step, not one per frame.
    pub drag_params: Option<EditParams>,
    /// The preset chip under the pointer, previewed on the canvas.
    pub hover_preset: Option<Preset>,
}

// ── Global egui style (called every frame from main.rs) ───────────────────────

pub fn apply_panel_style(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    // Transparent panel_fill so the wgpu image shows through the canvas area.
    style.visuals.panel_fill = Color32::TRANSPARENT;
    style.visuals.window_fill = SURFACE;
    style.visuals.override_text_color = Some(TEXT_1);
    // Widget colours (apply to egui's own widgets; custom buttons override per-call)
    style.visuals.widgets.inactive.bg_fill = RAISED;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = ACCENT_SOFT;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_LINE);
    style.visuals.widgets.active.bg_fill = ACCENT_PRESS;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.slider_trailing_fill = true;
    style.visuals.selection.bg_fill = ACCENT; // slider filled track
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    // Scrollbar
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.handle_min_length = 24.0;
    // Text fields, and a scroll track while it is shown, sit sunken.
    style.visuals.extreme_bg_color = SURFACE_SUNKEN;
    ctx.set_style(style);
}

// ── Button ────────────────────────────────────────────────────────────────────

/// What a button does, which decides its colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The one default action in a group, filled with the accent: Save with
    /// unsaved edits, a dialog's confirm.
    Primary,
    Secondary,
    /// Destroys or replaces something: Delete, Replace, Reset.
    Danger,
}

/// Size of a button. The rail's scales with the window (`T::btn`); dialogs
/// and chips are fixed.
#[derive(Clone, Copy)]
pub struct BtnSize {
    pub h: f32,
    pub font: f32,
    pub icon: f32,
    pub pad: f32,
    pub gap: f32,
    pub radius: f32,
    pub min_w: f32,
}

impl BtnSize {
    pub const DIALOG: Self = Self {
        h: 32.0,
        font: BODY,
        icon: 14.0,
        pad: 12.0,
        gap: 6.0,
        radius: 5.0,
        min_w: 72.0,
    };
    pub const CHIP: Self = Self {
        h: 26.0,
        font: CAPTION + 1.0,
        icon: 12.0,
        pad: 8.0,
        gap: 4.0,
        radius: 4.0,
        min_w: 0.0,
    };
}

/// A button, built with `Button::new(kind, label)` and the setters, then
/// drawn with `show`. Every button in Patina is one of these.
pub struct Button<'a> {
    kind: Kind,
    label: &'a str,
    icon: Option<&'a TextureHandle>,
    size: BtnSize,
    width: Option<f32>,
    selected: bool,
}

impl<'a> Button<'a> {
    pub fn new(kind: Kind, label: &'a str) -> Self {
        Self {
            kind,
            label,
            icon: None,
            size: BtnSize::DIALOG,
            width: None,
            selected: false,
        }
    }
    pub fn icon(mut self, icon: Option<&'a TextureHandle>) -> Self {
        self.icon = icon;
        self
    }
    pub fn size(mut self, size: BtnSize) -> Self {
        self.size = size;
        self
    }
    /// An exact width with the content centred; content-fitted otherwise.
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    /// A toggle or a choice that is on: Flip, the chosen format.
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let s = self.size;
        let font = egui::FontId::proportional(s.font);
        let galley = ui.fonts(|f| f.layout_no_wrap(self.label.to_string(), font, TEXT_1));
        let lbl_w = galley.size().x;
        let ic_w = match (self.icon.is_some(), self.label.is_empty()) {
            (false, _) => 0.0,
            (true, true) => s.icon,
            (true, false) => s.icon + s.gap,
        };
        let content_w = ic_w + lbl_w;
        let w = self.width.unwrap_or((content_w + 2.0 * s.pad).max(s.min_w));
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, s.h), Sense::click());
        if !ui.is_rect_visible(rect) {
            return resp;
        }

        let state = BtnState::from_resp(&resp, ui);
        let (bg, fg, border) = btn_colors(self.kind, state, self.selected);
        let p = ui.painter();
        let r = Rounding::same(s.radius);
        p.rect_filled(rect, r, bg);
        if let Some(c) = border {
            p.rect_stroke(rect, r, Stroke::new(1.0, c));
        }
        if resp.has_focus() {
            p.rect_stroke(
                rect.expand(2.0),
                Rounding::same(s.radius + 2.0),
                Stroke::new(2.0, ACCENT_LINE),
            );
        }

        let mut x = rect.center().x - content_w * 0.5;
        if let Some(tex) = self.icon {
            let ir = Rect::from_min_size(
                egui::pos2(x, rect.center().y - s.icon * 0.5),
                Vec2::splat(s.icon),
            );
            p.image(
                tex.id(),
                ir,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                fg,
            );
            x += ic_w;
        }
        p.galley_with_override_text_color(
            egui::pos2(x, rect.center().y - galley.size().y * 0.5),
            galley,
            fg,
        );
        resp
    }
}

/// The visual state of a button, derived from its egui response.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BtnState {
    Rest,
    Hover,
    Press,
    Disabled,
}

impl BtnState {
    fn from_resp(resp: &Response, ui: &Ui) -> Self {
        if !ui.is_enabled() {
            Self::Disabled
        } else if resp.is_pointer_button_down_on() {
            Self::Press
        } else if resp.hovered() {
            Self::Hover
        } else {
            Self::Rest
        }
    }
}

/// Fill, text and outline for every button there is.
fn btn_colors(kind: Kind, state: BtnState, selected: bool) -> (Color32, Color32, Option<Color32>) {
    use BtnState::*;
    match (kind, state) {
        (_, Disabled) => (SURFACE, TEXT_DISABLED, Some(BORDER)),
        (Kind::Primary, Rest) => (ACCENT, ON_ACCENT, None),
        (Kind::Primary, Hover) => (ACCENT_HOVER, ON_ACCENT, None),
        (Kind::Primary, Press) => (ACCENT_PRESS, ON_ACCENT, None),
        (Kind::Danger, Rest) => (RAISED, DANGER, Some(BORDER)),
        (Kind::Danger, Hover) => (DANGER_SOFT, DANGER, Some(DANGER_LINE)),
        (Kind::Danger, Press) => (DANGER_SOFT, DANGER, Some(DANGER)),
        (Kind::Secondary, _) if selected => (ACCENT_SOFT, ACCENT, Some(ACCENT_LINE)),
        (Kind::Secondary, Rest) => (RAISED, TEXT_1, Some(BORDER)),
        (Kind::Secondary, Hover) => (RAISED_HOVER, TEXT_1, Some(BORDER_STRONG)),
        (Kind::Secondary, Press) => (ACCENT_SOFT, ACCENT, Some(ACCENT_LINE)),
    }
}

// ── Panel entry point ─────────────────────────────────────────────────────────

/// What the rail needs to draw itself.
pub struct PanelInput<'a> {
    pub params: &'a EditParams,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Unsaved edits: Save is the primary action only then. On an untouched
    /// image it was lit green and meant nothing.
    pub dirty: bool,
    pub icons: &'a Icons,
    /// Of what the canvas shows: the edit, or the original while it is held.
    pub histogram: &'a Histogram,
}

/// The editor rail, organised around the edit: one toolbar, then presets,
/// geometry and every slider, all open — nothing to expand, and it fits a
/// 1200 px window without scrolling.
pub fn draw_panel(ctx: &Context, input: PanelInput<'_>) -> PanelAction {
    let PanelInput {
        params,
        can_undo,
        can_redo,
        dirty,
        icons,
        histogram,
    } = input;
    let mut action = PanelAction::default();
    let t = T::new(ctx);

    egui::SidePanel::right("editor_rail")
        .resizable(false)
        .exact_width(t.rail_w)
        .frame(Frame::none().fill(SURFACE).stroke(Stroke::new(1.0, BORDER)))
        .show(ctx, |ui| {
            ui.set_width(t.rail_w);
            toolbar(ui, &t, icons, can_undo, can_redo, dirty, &mut action);
            hairline(ui);
            draw_histogram(ui, &t, histogram);
            hairline(ui);

            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .drag_to_scroll(false)
                .show(ui, |ui| {
                    Frame::none()
                        .inner_margin(Margin::symmetric(t.row_x, t.row_y * 0.5))
                        .show(ui, |ui| {
                            ui.set_width(t.rail_w - 2.0 * t.row_x);
                            ui.spacing_mut().item_spacing = Vec2::new(t.gap_t, t.gap_t);
                            let mut p = params.clone();
                            presets(ui, &t, &mut p, &mut action);
                            geometry(ui, &t, icons, &p, &mut action);
                            for group in GROUPS {
                                slider_group(ui, &t, group, &mut p, &mut action);
                            }
                        });
                });
        });
    action
}

/// The rail head: red, green and blue levels, 0 on the left, 255 on the
/// right. All three share one scale, the tallest level between the ends: a
/// clipped spike at 0 or 255 runs to the top instead of flattening the rest.
fn draw_histogram(ui: &mut Ui, t: &T, h: &Histogram) {
    let height = (t.u * 3.0).round();
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height + 2.0 * t.gap),
        Sense::hover(),
    );
    let r = row.shrink2(Vec2::new(t.row_x, t.gap));
    let p = ui.painter();
    p.rect_filled(r, Rounding::same(t.radius), SURFACE_SUNKEN);
    let peak = [&h.r, &h.g, &h.b]
        .iter()
        .flat_map(|c| c[1..255].iter())
        .copied()
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let bin_w = r.width() / 256.0;
    for (bins, colour) in [(&h.r, HIST_R), (&h.g, HIST_G), (&h.b, HIST_B)] {
        let mut mesh = egui::Mesh::default();
        for (i, &n) in bins.iter().enumerate() {
            let v = (n as f32 / peak).min(1.0);
            if v > 0.0 {
                let x = r.min.x + i as f32 * bin_w;
                mesh.add_colored_rect(
                    Rect::from_min_max(
                        egui::pos2(x, r.max.y - v * r.height()),
                        egui::pos2(x + bin_w, r.max.y),
                    ),
                    colour,
                );
            }
        }
        p.add(mesh);
    }
}

/// Undo and Redo on the left; Save As, Save and its menu on the right.
fn toolbar(
    ui: &mut Ui,
    t: &T,
    icons: &Icons,
    can_undo: bool,
    can_redo: bool,
    dirty: bool,
    action: &mut PanelAction,
) {
    let h = 44.0_f32.max(t.btn_h + 2.0 * t.gap_t);
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let inner = row.shrink2(Vec2::new(t.row_x, 0.0));
    ui.allocate_ui_at_rect(inner, |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = t.gap_t;
            ui.add_enabled_ui(can_undo, |ui| {
                let undo = Button::new(Kind::Secondary, "")
                    .icon(Some(&icons.undo))
                    .size(t.btn())
                    .show(ui)
                    .on_hover_text("Undo  Ctrl+Z");
                action.undo |= undo.clicked();
            });
            ui.add_enabled_ui(can_redo, |ui| {
                let redo = Button::new(Kind::Secondary, "")
                    .icon(Some(&icons.redo))
                    .size(t.btn())
                    .show(ui)
                    .on_hover_text("Redo  Ctrl+Shift+Z");
                action.redo |= redo.clicked();
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = t.gap_t;
                let kind = if dirty {
                    Kind::Primary
                } else {
                    Kind::Secondary
                };
                let more = Button::new(kind, "")
                    .icon(Some(&icons.chevron))
                    .size(t.btn())
                    .show(ui)
                    .on_hover_text("More save actions");
                let save = Button::new(kind, "Save")
                    .icon(Some(&icons.save))
                    .size(t.btn())
                    .show(ui)
                    .on_hover_text("Save  Ctrl+S");
                action.save |= save.clicked();
                let save_as = Button::new(Kind::Secondary, "Save As")
                    .icon(Some(&icons.save_as))
                    .size(t.btn())
                    .show(ui)
                    .on_hover_text("Save As  Ctrl+Shift+S");
                action.save_as |= save_as.clicked();

                let menu = ui.make_persistent_id("save_menu");
                if more.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(menu));
                }
                egui::popup::popup_below_widget(ui, menu, &more, |ui| {
                    ui.set_min_width(170.0);
                    let reset = Button::new(Kind::Danger, "Reset all edits")
                        .icon(Some(&icons.reset))
                        .size(t.btn())
                        .show(ui);
                    if reset.clicked() {
                        action.reset_all = true;
                        ui.memory_mut(|m| m.close_popup());
                    }
                });
            });
        });
    });
}

/// A group's heading. Its dot appears only when something in the group is
/// changed, and clicking it resets the group; before, every heading carried
/// a glowing dot that meant nothing. Returns whether the dot was clicked.
fn group_head(ui: &mut Ui, t: &T, title: &str, modified: bool) -> bool {
    ui.add_space(t.gap_t);
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), t.caption * 2.0),
        Sense::hover(),
    );
    ui.painter().text(
        row.left_center(),
        egui::Align2::LEFT_CENTER,
        title.to_uppercase(),
        egui::FontId::proportional(t.caption),
        TEXT_3,
    );
    if !modified {
        return false;
    }
    let dot = Rect::from_center_size(
        egui::pos2(row.max.x - t.dot, row.center().y),
        Vec2::splat(t.dot * 2.0),
    );
    let resp = ui
        .interact(dot, ui.make_persistent_id(("reset", title)), Sense::click())
        .on_hover_text(format!("Reset {}", title.to_lowercase()));
    let c = if resp.hovered() { ACCENT_HOVER } else { ACCENT };
    ui.painter().circle_filled(dot.center(), t.dot * 0.4, c);
    resp.clicked()
}

fn hairline(ui: &mut Ui) {
    let r = ui.available_rect_before_wrap();
    ui.painter()
        .hline(r.x_range(), r.top(), Stroke::new(1.0, BORDER));
    ui.add_space(1.0);
}

// ── Presets ───────────────────────────────────────────────────────────────────

/// Text chips in the order of their families. A chip previews on the canvas
/// while hovered; a click makes it one undo step, and a click on the chosen
/// one goes back to none. They replace twelve thumbnail tiles that were all
/// the same placeholder picture.
fn presets(ui: &mut Ui, t: &T, p: &mut EditParams, action: &mut PanelAction) {
    if group_head(ui, t, "Presets", p.preset != Preset::None) {
        p.preset = Preset::None;
        action.new_params = Some(p.clone());
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(t.gap_t);
        for &preset in PRESET_ORDER {
            let chosen = p.preset == preset;
            let resp = Button::new(Kind::Secondary, preset.label())
                .size(BtnSize {
                    font: t.body,
                    ..BtnSize::CHIP
                })
                .selected(chosen)
                .show(ui);
            if resp.hovered() && !chosen {
                action.hover_preset = Some(preset);
            }
            if resp.clicked() {
                p.preset = if chosen { Preset::None } else { preset };
                action.new_params = Some(p.clone());
                action.hover_preset = None;
            }
        }
    });
}

/// Quick fixes, bright, film, dramatic, black and white.
const PRESET_ORDER: &[Preset] = &[
    Preset::AutoEnhance,
    Preset::Vivid,
    Preset::Punch,
    Preset::CleanBright,
    Preset::SoftGlow,
    Preset::WarmFilm,
    Preset::GoldenHour,
    Preset::FadedMatte,
    Preset::Moody,
    Preset::Chrome,
    Preset::Bw,
    Preset::BwContrast,
];

// ── Geometry ──────────────────────────────────────────────────────────────────

/// Rotate and flip as one row of four icon buttons.
fn geometry(ui: &mut Ui, t: &T, icons: &Icons, p: &EditParams, action: &mut PanelAction) {
    let changed = p.rotation != crate::editor::Rotation::R0 || p.flip_h || p.flip_v;
    if group_head(ui, t, "Geometry", changed) {
        let mut q = p.clone();
        q.rotation = crate::editor::Rotation::R0;
        q.flip_h = false;
        q.flip_v = false;
        action.new_params = Some(q);
    }
    let w = (ui.available_width() - 3.0 * t.gap_t) / 4.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = t.gap_t;
        let mut btn = |icon: &TextureHandle, tip: &str, on: bool| {
            Button::new(Kind::Secondary, "")
                .icon(Some(icon))
                .size(t.btn())
                .width(w)
                .selected(on)
                .show(ui)
                .on_hover_text(tip)
                .clicked()
        };
        action.rot_ccw |= btn(&icons.rotate_left, "Rotate left", false);
        action.rot_cw |= btn(&icons.rotate_right, "Rotate right", false);
        action.flip_h |= btn(&icons.flip_h, "Flip horizontally", p.flip_h);
        action.flip_v |= btn(&icons.flip_v, "Flip vertically", p.flip_v);
    });
}

// ── Sliders ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Field {
    Exposure,
    Highlights,
    Shadows,
    Brightness,
    Contrast,
    Saturation,
    Vibrance,
    Sharpen,
}

impl Field {
    fn of(self, p: &mut EditParams) -> &mut f32 {
        match self {
            Field::Exposure => &mut p.exposure,
            Field::Highlights => &mut p.highlights,
            Field::Shadows => &mut p.shadows,
            Field::Brightness => &mut p.brightness,
            Field::Contrast => &mut p.contrast,
            Field::Saturation => &mut p.saturation,
            Field::Vibrance => &mut p.vibrance,
            Field::Sharpen => &mut p.sharpen,
        }
    }
}

struct SliderDef {
    label: &'static str,
    field: Field,
    lo: f32,
    hi: f32,
}

struct Group {
    title: &'static str,
    sliders: &'static [SliderDef],
}

const fn s(label: &'static str, field: Field, lo: f32, hi: f32) -> SliderDef {
    SliderDef {
        label,
        field,
        lo,
        hi,
    }
}

/// Every slider, always open. Zero is each one's default.
const GROUPS: &[Group] = &[
    Group {
        title: "Light",
        sliders: &[
            s("Exposure", Field::Exposure, -2.0, 2.0),
            s("Highlights", Field::Highlights, -1.0, 1.0),
            s("Shadows", Field::Shadows, -1.0, 1.0),
            s("Brightness", Field::Brightness, -1.0, 1.0),
            s("Contrast", Field::Contrast, -1.0, 1.0),
        ],
    },
    Group {
        title: "Color",
        sliders: &[
            s("Saturation", Field::Saturation, -1.0, 1.0),
            s("Vibrance", Field::Vibrance, -1.0, 1.0),
        ],
    },
    Group {
        title: "Detail",
        sliders: &[s("Sharpen", Field::Sharpen, 0.0, 1.0)],
    },
];

fn slider_group(ui: &mut Ui, t: &T, g: &Group, p: &mut EditParams, action: &mut PanelAction) {
    let modified = g.sliders.iter().any(|d| d.field.of(p).abs() > CHANGED);
    if group_head(ui, t, g.title, modified) {
        for d in g.sliders {
            *d.field.of(p) = 0.0;
        }
        action.new_params = Some(p.clone());
    }
    for d in g.sliders {
        match slider(ui, t, d.label, d.field.of(p), d.lo, d.hi).0 {
            Change::None => {}
            Change::Step => action.new_params = Some(p.clone()),
            Change::Continue => action.drag_params = Some(p.clone()),
        }
    }
}

/// Below this a value counts as its default, zero.
const CHANGED: f32 = 1e-4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Change {
    None,
    /// A new undo step.
    Step,
    /// Continues the step the current drag or key-repeat began.
    Continue,
}

/// One slider row: label, track, value. Drag to set (Shift for a tenth of
/// the speed), double-click to reset, ←/→ when focused, or click the value
/// to type one. Painted here rather than egui's Slider so a drag can report
/// where it begins: egui's reports a change every frame, and each became an
/// undo step, so one drag filled the whole fifty-step history.
/// Returns the change and the track's rect.
fn slider(ui: &mut Ui, t: &T, label: &str, value: &mut f32, lo: f32, hi: f32) -> (Change, Rect) {
    let id = ui.make_persistent_id(("slider", label));
    let (row, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), t.slider_h), Sense::hover());
    let value_r = Rect::from_min_max(egui::pos2(row.max.x - t.sval_w, row.min.y), row.max);
    let track_r = Rect::from_min_max(
        egui::pos2(row.min.x + t.slbl_w, row.min.y),
        egui::pos2(value_r.min.x - t.gap, row.max.y),
    );
    let span = hi - lo;
    let before = *value;
    let mut continues = false;

    // Drag, double-click and keys on the track.
    let resp = ui.interact(track_r, id, Sense::click_and_drag());
    if resp.double_clicked() {
        *value = 0.0;
    } else if resp.dragged() {
        let shift = ui.input(|i| i.modifiers.shift);
        if shift {
            *value += resp.drag_delta().x / track_r.width() * span * 0.1;
        } else if let Some(pos) = resp.interact_pointer_pos() {
            *value = lo + ((pos.x - track_r.min.x) / track_r.width()).clamp(0.0, 1.0) * span;
        }
        // The first change of a drag is the step; the rest continue it.
        continues = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    }
    if resp.has_focus() {
        ui.input_mut(|i| {
            let fine = i.modifiers.shift;
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key: k @ (egui::Key::ArrowLeft | egui::Key::ArrowRight),
                    pressed: true,
                    repeat,
                    ..
                } => {
                    let step = span / 100.0 * if fine { 0.1 } else { 1.0 };
                    let dir = if *k == egui::Key::ArrowLeft {
                        -1.0
                    } else {
                        1.0
                    };
                    *value += dir * step;
                    continues |= *repeat;
                    false
                }
                _ => true,
            });
        });
    }

    // Typing a value: click it, Enter to set, Esc to leave it.
    let edit_id = id.with("typed");
    let typing = ui.data(|d| d.get_temp::<String>(edit_id));
    if let Some(mut text) = typing {
        let te = ui.put(
            value_r,
            egui::TextEdit::singleline(&mut text)
                .font(egui::FontId::monospace(t.caption))
                .horizontal_align(egui::Align::Max),
        );
        if !te.has_focus() && !te.lost_focus() {
            te.request_focus();
        }
        let esc = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if te.lost_focus() || esc {
            ui.data_mut(|d| d.remove::<String>(edit_id));
            if !esc {
                if let Ok(v) = text
                    .trim()
                    .replace('\u{2212}', "-")
                    .replace('+', "")
                    .parse::<f32>()
                {
                    *value = v.clamp(lo, hi);
                }
            }
        } else {
            ui.data_mut(|d| d.insert_temp(edit_id, text));
        }
    } else {
        let vr = ui
            .interact(value_r, id.with("value"), Sense::click())
            .on_hover_text("Click to type a value");
        if vr.clicked() {
            ui.data_mut(|d| d.insert_temp(edit_id, format!("{:.2}", *value)));
        }
        let modified = value.abs() > CHANGED;
        ui.painter().text(
            value_r.right_center(),
            egui::Align2::RIGHT_CENTER,
            format_slider_value(*value, lo < 0.0),
            egui::FontId::monospace(t.caption),
            if modified { TEXT_1 } else { TEXT_3 },
        );
    }

    *value = value.clamp(lo, hi);
    let change = if (*value - before).abs() <= 1e-6 {
        Change::None
    } else if continues {
        Change::Continue
    } else {
        Change::Step
    };
    if resp.dragged() && change != Change::None {
        ui.data_mut(|d| d.insert_temp(id, true));
    } else if !resp.dragged() {
        ui.data_mut(|d| d.remove::<bool>(id));
    }

    // Paint: the label, a track filled from zero to the value, the knob.
    let modified = value.abs() > CHANGED;
    let p = ui.painter();
    p.text(
        row.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(t.body),
        if modified { TEXT_1 } else { TEXT_2 },
    );
    let x = |v: f32| track_r.min.x + (v - lo) / span * track_r.width();
    let cy = track_r.center().y;
    let line = Rect::from_min_max(
        egui::pos2(track_r.min.x, cy - 2.0),
        egui::pos2(track_r.max.x, cy + 2.0),
    );
    p.rect_filled(line, Rounding::same(2.0), BORDER);
    let (a, b) = (x(0.0_f32.clamp(lo, hi)), x(*value));
    let fill = Rect::from_min_max(
        egui::pos2(a.min(b), cy - 2.0),
        egui::pos2(a.max(b), cy + 2.0),
    );
    if fill.width() >= 0.5 {
        p.rect_filled(fill, Rounding::same(2.0), ACCENT);
    }
    let knob = egui::pos2(b, cy);
    if resp.has_focus() {
        p.circle_stroke(knob, 9.0, Stroke::new(2.0, ACCENT_LINE));
    }
    let active = resp.hovered() || resp.dragged();
    p.circle_filled(knob, if active { 7.0 } else { 6.0 }, TEXT_1);
    (change, track_r)
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
        format!("\u{2212}{:.2}", value.abs()) // Unicode minus
    }
}

// ── Info card ─────────────────────────────────────────────────────────────────

/// The file's EXIF, in a card over the canvas's top-left corner (I). It was a
/// section of the editor rail, where the viewer — where you look a photo
/// over — could not show it.
pub fn draw_info_card(ctx: &Context, metadata: Option<&crate::exif::ImageMetadata>) {
    egui::Area::new(egui::Id::new("info_card"))
        .fixed_pos(egui::pos2(16.0, 16.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::none()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .rounding(Rounding::same(8.0))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.set_max_width(320.0);
                    ui.spacing_mut().item_spacing = Vec2::new(12.0, 5.0);
                    match metadata.filter(|m| !m.is_empty()) {
                        None => {
                            ui.label(RichText::new("No EXIF data").color(TEXT_3).size(BODY));
                        }
                        Some(meta) => {
                            egui::Grid::new("info_grid").num_columns(2).show(ui, |ui| {
                                for field in &meta.fields {
                                    ui.label(
                                        RichText::new(field.label).color(TEXT_3).size(CAPTION),
                                    );
                                    ui.label(RichText::new(&field.value).color(TEXT_1).size(BODY));
                                    ui.end_row();
                                }
                            });
                        }
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one slider through egui with real pointer events.
    fn frame(ctx: &Context, value: &mut f32, events: Vec<egui::Event>) -> (Change, Rect) {
        let mut out = (Change::None, Rect::NOTHING);
        let raw = egui::RawInput {
            events,
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let t = T::new(ctx);
                out = slider(ui, &t, "Exposure", value, -2.0, 2.0);
            });
        });
        out
    }

    fn pointer(pos: egui::Pos2, pressed: Option<bool>) -> Vec<egui::Event> {
        let mut ev = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            ev.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        ev
    }

    /// One drag is one undo step: its first change is a Step, every later
    /// frame of it continues that step.
    #[test]
    fn a_drag_is_one_step() {
        let ctx = Context::default();
        install_fonts(&ctx);
        let mut v = 0.0;
        let (_, track) = frame(&ctx, &mut v, vec![]);
        let y = track.center().y;
        let at = |f: f32| egui::pos2(track.min.x + f * track.width(), y);

        let mut changes = vec![];
        changes.push(frame(&ctx, &mut v, pointer(at(0.5), Some(true))).0);
        for f in [0.55, 0.6, 0.7, 0.8, 0.9] {
            changes.push(frame(&ctx, &mut v, pointer(at(f), None)).0);
        }
        changes.push(frame(&ctx, &mut v, pointer(at(0.9), Some(false))).0);

        let steps = changes.iter().filter(|c| **c == Change::Step).count();
        let conts = changes.iter().filter(|c| **c == Change::Continue).count();
        assert_eq!(steps, 1, "{changes:?}");
        assert!(conts >= 3, "{changes:?}");
        assert!((v - 1.6).abs() < 0.05, "ended near 90% of −2..2: {v}");

        // The next drag is a new step.
        frame(&ctx, &mut v, pointer(at(0.9), Some(true)));
        let next = frame(&ctx, &mut v, pointer(at(0.3), None)).0;
        assert_eq!(next, Change::Step);
    }

    #[test]
    fn the_rail_offers_every_preset_once() {
        let mut want: Vec<_> = Preset::ALL.iter().filter(|p| **p != Preset::None).collect();
        let mut got: Vec<_> = PRESET_ORDER.iter().collect();
        want.sort_by_key(|p| p.label());
        got.sort_by_key(|p| p.label());
        assert_eq!(got, want);
    }

    #[test]
    fn values_read_with_a_real_minus() {
        assert_eq!(format_slider_value(-0.5, true), "\u{2212}0.50");
        assert_eq!(format_slider_value(0.001, true), "0");
        assert_eq!(format_slider_value(0.25, false), "0.25");
    }
}
