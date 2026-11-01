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
    pub check: TextureHandle,
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
                Err(_) => egui::ColorImage::new([108, 81], SURFACE_SUNKEN),
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
    style.visuals.extreme_bg_color = Color32::TRANSPARENT;
    ctx.set_style(style);
}

// ── Panel entry point ─────────────────────────────────────────────────────────

pub fn draw_panel(
    ctx: &Context,
    params: &EditParams,
    can_undo: bool,
    can_redo: bool,
    // Save is the primary action only when there is something to save; on
    // an untouched image it was lit green and meant nothing.
    dirty: bool,
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
        .frame(Frame::none().fill(SURFACE).stroke(Stroke::new(1.0, BORDER)))
        .show(ctx, |ui| {
            ui.set_width(t.rail_w);

            header_row(ui, &t, "FILE", |ui| {
                if Button::new(
                    if dirty {
                        Kind::Primary
                    } else {
                        Kind::Secondary
                    },
                    "Save",
                )
                .icon(Some(&icons.save))
                .size(t.btn())
                .show(ui)
                .clicked()
                {
                    action.save = true;
                }
                if Button::new(Kind::Secondary, "Save As")
                    .icon(Some(&icons.save_as))
                    .size(t.btn())
                    .show(ui)
                    .clicked()
                {
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
                    if Button::new(Kind::Danger, labels.2)
                        .icon(Some(&icons.reset))
                        .size(t.btn())
                        .show(ui)
                        .clicked()
                    {
                        action.reset_all = true;
                    }
                    ui.add_enabled_ui(can_redo, |ui| {
                        if Button::new(Kind::Secondary, labels.1)
                            .icon(Some(&icons.redo))
                            .size(t.btn())
                            .show(ui)
                            .clicked()
                        {
                            action.redo = true;
                        }
                    });
                    ui.add_enabled_ui(can_undo, |ui| {
                        if Button::new(Kind::Secondary, labels.0)
                            .icon(Some(&icons.undo))
                            .size(t.btn())
                            .show(ui)
                            .clicked()
                        {
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

// ── Structural layout helpers ─────────────────────────────────────────────────

/// FILE / HISTORY rows: faint dot + fixed label + inline buttons, same height as sections.
fn header_row(ui: &mut Ui, t: &T, label: &str, add_buttons: impl FnOnce(&mut Ui)) {
    let row_h = t.btn_h + t.row_y * 2.0;
    let avail_w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, row_h), Sense::hover());
    let cy = rect.center().y;

    let dot_cx = rect.min.x + t.row_x + t.dot;
    ui.painter()
        .circle_filled(egui::pos2(dot_cx, cy), t.dot * 0.5, TEXT_3);
    ui.painter().text(
        egui::pos2(dot_cx + t.dot + t.gap_t, cy),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(t.caption),
        TEXT_1,
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
        pt.rect_filled(rect, Rounding::ZERO, RAISED);
    }

    // Spruce dot with halo
    let cx = rect.min.x + t.row_x + t.dot;
    pt.circle_filled(egui::pos2(cx, cy), t.dot * 0.83, ACCENT_SOFT);
    pt.circle_filled(egui::pos2(cx, cy), t.dot * 0.50, ACCENT);

    pt.text(
        egui::pos2(cx + t.dot * 1.5 + t.gap_t * 0.5, cy),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(t.caption),
        TEXT_1,
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
            TEXT_2,
        );
    } else {
        let xi = ic_rect.min.x + ic_sz.x * 0.3;
        let xo = ic_rect.max.x - ic_sz.x * 0.3;
        let mk = Stroke::new(1.5, TEXT_2);
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
    let lbl_h = t.row_y * 0.5 + t.body + t.row_y * 0.5;
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
                        .color(TEXT_3)
                        .size(t.caption)
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
                    ACCENT_SOFT
                } else {
                    SURFACE_SUNKEN
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
                        ON_ACCENT,
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
                    SURFACE_SUNKEN,
                );
                p.text(
                    lbl_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    preset.label(),
                    egui::FontId::proportional(t.body),
                    if active || resp.hovered() {
                        ACCENT
                    } else {
                        TEXT_1
                    },
                );

                // Border: solid accent when active, spruce when hovered, dim when rest
                let (sc, sw) = if active {
                    (ACCENT, 1.5)
                } else if resp.hovered() {
                    (ACCENT_LINE, 1.0)
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
                if Button::new(Kind::Secondary, "Rotate left")
                    .icon(Some(&icons.rotate_left))
                    .size(t.btn())
                    .width(btn_w)
                    .show(ui)
                    .clicked()
                {
                    action.rot_ccw = true;
                }
                if Button::new(Kind::Secondary, "Rotate right")
                    .icon(Some(&icons.rotate_right))
                    .size(t.btn())
                    .width(btn_w)
                    .show(ui)
                    .clicked()
                {
                    action.rot_cw = true;
                }
            });
            ui.add_space(t.gap);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = t.gap;
                if Button::new(Kind::Secondary, "Flip H")
                    .icon(Some(&icons.flip_h))
                    .size(t.btn())
                    .selected(p.flip_h)
                    .width(btn_w)
                    .show(ui)
                    .clicked()
                {
                    action.flip_h = true;
                }
                if Button::new(Kind::Secondary, "Flip V")
                    .icon(Some(&icons.flip_v))
                    .size(t.btn())
                    .selected(p.flip_v)
                    .width(btn_w)
                    .show(ui)
                    .clicked()
                {
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
                    egui::FontId::proportional(t.body),
                    TEXT_2,
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
                    egui::FontId::proportional(t.body),
                    TEXT_1,
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
                        egui::FontId::proportional(t.body),
                        TEXT_2,
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
                        egui::FontId::monospace(t.caption),
                        TEXT_1,
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
                            .color(TEXT_3)
                            .size(t.body),
                    );
                }
                Some(meta) if meta.is_empty() => {
                    ui.label(
                        RichText::new("No EXIF data available")
                            .color(TEXT_3)
                            .size(t.body),
                    );
                }
                Some(meta) => {
                    ui.spacing_mut().item_spacing = egui::Vec2::new(0.0, 5.0);
                    for field in &meta.fields {
                        ui.horizontal(|ui| {
                            // Label column: fixed width, dimmed
                            let (_, lr) = ui.allocate_space(egui::Vec2::new(
                                t.slbl_w + t.sval_w * 0.5,
                                t.body * 1.6,
                            ));
                            ui.painter().text(
                                lr.left_center(),
                                egui::Align2::LEFT_CENTER,
                                field.label,
                                egui::FontId::proportional(t.caption),
                                TEXT_3,
                            );
                            // Value: wrapping, full brightness
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(&field.value).color(TEXT_1).size(t.body),
                                    );
                                },
                            );
                        });
                    }
                }
            }
        });
}
