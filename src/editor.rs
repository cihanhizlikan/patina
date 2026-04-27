//! editor.rs — Non-destructive image editing pipeline for Patina.
//!
//! Architecture
//! ============
//! The pipeline is strictly one-way:
//!
//!   original (DecodedImage, immutable)
//!       ↓  apply_edits()
//!   display (DecodedImage, recomputed from scratch on each param change)
//!
//! `EditParams` is a plain value struct — cheap to clone — so the undo stack
//! is simply a `Vec<EditParams>` with no pixel copying.
//!
//! Operations are applied per-frame in this fixed order:
//!   1. Rotation → Flips → Scale
//!   2. Exposure → Highlights/Shadows → Brightness → Contrast
//!   3. Saturation → Vibrance
//!   4. Style flags from the active preset
//!   5. Sharpening
//!
//! Animated GIFs are edited frame-by-frame; every frame receives the same params.

use crate::loader::{DecodedImage, Frame};

// ── Rotation ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Rotation { #[default] R0, R90, R180, R270 }

impl Rotation {
    pub fn cw(self) -> Self {
        match self { Self::R0=>Self::R90, Self::R90=>Self::R180,
                     Self::R180=>Self::R270, Self::R270=>Self::R0 }
    }
    pub fn ccw(self) -> Self {
        match self { Self::R0=>Self::R270, Self::R90=>Self::R0,
                     Self::R180=>Self::R90, Self::R270=>Self::R180 }
    }
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self { Self::R0=>"0°", Self::R90=>"90°",
                     Self::R180=>"180°", Self::R270=>"270°" }
    }
}

// ── Preset filters ────────────────────────────────────────────────────────────
//
// Presets are defined by the thumbnail zip: 12 images across 5 groups.
// No "Normal" preset — the viewer shows the unedited image by default.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Preset {
    // No filter — original image
    #[default] None,
    // Quick Fixes
    AutoEnhance,
    Punch,
    Vivid,
    // Dramatic
    Chrome,
    Moody,
    // Film
    FadedMatte,
    GoldenHour,
    WarmFilm,
    // Black & White
    Bw,
    BwContrast,
    // Bright & Airy
    CleanBright,
    SoftGlow,
}

impl Preset {
    pub const ALL: &'static [Self] = &[
        Self::None,
        Self::AutoEnhance, Self::Punch,     Self::Vivid,
        Self::Chrome,      Self::Moody,
        Self::FadedMatte,  Self::GoldenHour, Self::WarmFilm,
        Self::Bw,          Self::BwContrast,
        Self::CleanBright, Self::SoftGlow,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None        => "Normal",
            Self::AutoEnhance => "Auto Enhance",
            Self::Punch       => "Punch",
            Self::Vivid       => "Vivid",
            Self::Chrome      => "Chrome",
            Self::Moody       => "Moody",
            Self::FadedMatte  => "Faded Matte",
            Self::GoldenHour  => "Golden Hour",
            Self::WarmFilm    => "Warm Film",
            Self::Bw          => "B&W",
            Self::BwContrast  => "B&W Contrast",
            Self::CleanBright => "Clean Bright",
            Self::SoftGlow    => "Soft Glow",
        }
    }
}
// ── Edit parameters ───────────────────────────────────────────────────────────

/// Complete set of adjustments. Default = identity (no change).
#[derive(Clone, Debug)]
pub struct EditParams {
    // Geometry
    pub rotation:   Rotation,
    pub flip_h:     bool,
    pub flip_v:     bool,
    /// [0.05 – 4.0], 1.0 = original
    pub scale:      f32,
    // Tonal
    /// [-2.0 – +2.0] EV stops
    pub exposure:   f32,
    /// [-1.0 – +1.0]
    pub highlights: f32,
    /// [-1.0 – +1.0]
    pub shadows:    f32,
    /// [-1.0 – +1.0] linear offset
    pub brightness: f32,
    /// [-1.0 – +1.0] S-curve strength
    pub contrast:   f32,
    // Colour
    /// [-1.0 – +1.0]
    pub saturation: f32,
    /// [-1.0 – +1.0]
    pub vibrance:   f32,
    // Detail
    /// [0.0 – 1.0]
    pub sharpen:    f32,
    // Preset
    pub preset:     Preset,
}

impl Default for EditParams {
    fn default() -> Self {
        Self {
            rotation: Rotation::R0, flip_h: false, flip_v: false, scale: 1.0,
            exposure: 0.0, highlights: 0.0, shadows: 0.0,
            brightness: 0.0, contrast: 0.0,
            saturation: 0.0, vibrance: 0.0,
            sharpen: 0.0, preset: Preset::None,
        }
    }
}

impl EditParams {
    /// Returns `true` when applying these params to any image would be a no-op.
    /// Currently always false because presets can alter images even at "zero" sliders.
    /// Kept as an extension point for future short-circuit optimisation.
    pub fn is_identity(&self) -> bool { false }
}

// ── Undo stack ────────────────────────────────────────────────────────────────

const UNDO_LIMIT: usize = 50;

pub struct UndoStack {
    history: Vec<EditParams>,
    cursor:  usize,
}

impl UndoStack {
    pub fn new() -> Self { Self { history: vec![EditParams::default()], cursor: 0 } }
    pub fn current(&self)         -> &EditParams     { &self.history[self.cursor] }
    #[allow(dead_code)]
    pub fn current_mut(&mut self) -> &mut EditParams { &mut self.history[self.cursor] }
    #[allow(dead_code)]
    pub fn reset(&mut self)       { *self = Self::new(); }

    pub fn push(&mut self, p: EditParams) {
        self.history.truncate(self.cursor + 1);
        self.history.push(p);
        if self.history.len() > UNDO_LIMIT + 1 {
            self.history.remove(0);
        } else {
            self.cursor += 1;
        }
    }

    pub fn can_undo(&self) -> bool { self.cursor > 0 }
    pub fn can_redo(&self) -> bool { self.cursor + 1 < self.history.len() }
    pub fn undo(&mut self) { if self.can_undo() { self.cursor -= 1; } }
    pub fn redo(&mut self) { if self.can_redo() { self.cursor += 1; } }
}

// ── Editor state ──────────────────────────────────────────────────────────────

pub struct EditorState {
    pub undo:    UndoStack,
    /// Processed image kept in sync with `undo.current()`.
    pub display: DecodedImage,
    /// True when params changed and `display` needs recomputing.
    pub dirty:   bool,
}

impl EditorState {
    pub fn new(original: &DecodedImage) -> Self {
        Self { undo: UndoStack::new(), display: original.clone(), dirty: false }
    }

    /// Commit a new params snapshot and mark dirty.
    pub fn commit(&mut self, p: EditParams) {
        self.undo.push(p);
        self.dirty = true;
    }

    /// Commit params without triggering pixel reprocessing — for live slider drag.
    /// Call sync() later when the user pauses.
    #[allow(dead_code)] // Reserved for live-drag optimisation (no re-encode during drag)
    pub fn commit_no_sync(&mut self, p: EditParams) {
        self.undo.push(p);
        self.dirty = true;
        // Do not set self.display — caller will call sync() after debounce
    }

    /// Recompute `display` if dirty. Call once per redraw.
    #[allow(dead_code)]
    pub fn sync(&mut self, original: &DecodedImage) {
        if !self.dirty { return; }
        self.dirty   = false;
        self.display = apply_edits(original, self.undo.current());
    }

    #[allow(dead_code)]
    pub fn do_undo(&mut self, original: &DecodedImage) {
        self.undo.undo();
        self.dirty = true;
        self.sync(original);
    }
    #[allow(dead_code)]
    pub fn do_redo(&mut self, original: &DecodedImage) {
        self.undo.redo();
        self.dirty = true;
        self.sync(original);
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Produce a new `DecodedImage` by applying `params` to every frame of `src`.
pub fn apply_edits(src: &DecodedImage, params: &EditParams) -> DecodedImage {
    if params.is_identity() { return src.clone(); }

    let mut out_w = src.width;
    let mut out_h = src.height;

    let frames: Vec<Frame> = src.frames.iter().map(|f| {
        let (pixels, w, h) = process_frame(&f.pixels, src.width, src.height, params);
        out_w = w; out_h = h;
        Frame { pixels, delay_ms: f.delay_ms }
    }).collect();

    DecodedImage { frames, width: out_w, height: out_h }
}

// ── Per-frame pipeline ────────────────────────────────────────────────────────

fn process_frame(px: &[u8], w: u32, h: u32, p: &EditParams) -> (Vec<u8>, u32, u32) {
    // 1. Geometry
    let (px, w, h) = do_rotate(px, w, h, p.rotation);
    let (px, w, h) = do_flip(&px, w, h, p.flip_h, p.flip_v);
    // 2. Scale
    let (px, w, h) = if (p.scale - 1.0).abs() > 1e-4 {
        scale_bilinear(&px, w, h, p.scale)
    } else { (px, w, h) };
    // 3–4. Tonal + colour + style
    let pp = resolve_pipeline(p, &px, w, h);
    let px = tonal_colour(&px, &pp);
    // 5. Sharpen
    let px = if pp.sharpen > 1e-4 { unsharp_mask(&px, w, h, pp.sharpen) } else { px };
    (px, w, h)
}

// ── Pipeline parameter resolution ─────────────────────────────────────────────

struct PipelineParams {
    exposure: f32, highlights: f32, shadows: f32,
    brightness: f32, contrast: f32,
    saturation: f32, vibrance: f32,
    sharpen: f32,
    bw_warm: bool, vintage_fade: bool,
    warm_analog: bool, cool_film: bool, soft_glow: bool,
}

fn resolve_pipeline(p: &EditParams, pixels: &[u8], _w: u32, _h: u32) -> PipelineParams {
    /// Base pipeline from EditParams — all style flags off.
    fn base(p: &EditParams) -> PipelineParams {
        PipelineParams {
            exposure: p.exposure, highlights: p.highlights, shadows: p.shadows,
            brightness: p.brightness, contrast: p.contrast,
            saturation: p.saturation, vibrance: p.vibrance,
            sharpen: p.sharpen,
            bw_warm: false, vintage_fade: false,
            warm_analog: false, cool_film: false, soft_glow: false,
        }
    }

    // None preset = sliders only, no colour grading
    if p.preset == Preset::None { return base(p); }
    let mut o = base(p);
    match p.preset {
        Preset::AutoEnhance => {
            let (exp, br, sat) = auto_enhance(pixels);
            o.exposure    = (o.exposure   + exp).clamp(-2.0, 2.0);
            o.brightness  = (o.brightness + br ).clamp(-1.0, 1.0);
            o.contrast    = o.contrast * 0.5 + 0.15;
            o.saturation  = (o.saturation + sat).clamp(-1.0, 1.0);
        }
        Preset::Punch => {
            // Strong contrast + sharpening — punchy, dramatic pop
            o.contrast    = o.contrast * 0.5 + 0.40;
            o.saturation  = (o.saturation + 0.35).clamp(-1.0, 1.0);
            o.vibrance    = (o.vibrance   + 0.20).clamp(-1.0, 1.0);
            o.highlights  = (o.highlights - 0.12).clamp(-1.0, 1.0);
            o.sharpen     = (o.sharpen    + 0.30).clamp(0.0, 1.0);
        }
        Preset::Vivid => {
            // Rich saturated colours, moderate contrast
            o.contrast    = o.contrast * 0.5 + 0.20;
            o.saturation  = (o.saturation + 0.45).clamp(-1.0, 1.0);
            o.vibrance    = (o.vibrance   + 0.30).clamp(-1.0, 1.0);
        }
        Preset::Chrome => {
            // High contrast, saturated blues/reds, clean whites
            o.contrast    = o.contrast * 0.5 + 0.30;
            o.saturation  = (o.saturation + 0.35).clamp(-1.0, 1.0);
            o.highlights  = (o.highlights - 0.15).clamp(-1.0, 1.0);
            o.vibrance    = (o.vibrance   + 0.15).clamp(-1.0, 1.0);
            o.sharpen     = (o.sharpen    + 0.15).clamp(0.0, 1.0);
        }
        Preset::Moody => {
            // Cinematic: crushed blacks, desaturated, dramatic
            o.exposure    = (o.exposure   - 0.30).clamp(-2.0, 2.0);
            o.contrast    = o.contrast * 0.5 + 0.30;
            o.saturation  = (o.saturation - 0.25).clamp(-1.0, 1.0);
            o.shadows     = (o.shadows    - 0.10).clamp(-1.0, 1.0);
            o.highlights  = (o.highlights - 0.08).clamp(-1.0, 1.0);
        }
        Preset::FadedMatte => {
            // Lifted blacks, muted — classic faded film look
            o.exposure    = (o.exposure   - 0.10).clamp(-2.0, 2.0);
            o.contrast    = o.contrast * 0.5 - 0.20;
            o.saturation  = (o.saturation - 0.25).clamp(-1.0, 1.0);
            o.shadows     = (o.shadows    + 0.18).clamp(-1.0, 1.0);
            o.vintage_fade = true;
        }
        Preset::GoldenHour => {
            // Warm orange-gold tones, slightly lifted, analog feel
            o.exposure    = (o.exposure   + 0.10).clamp(-2.0, 2.0);
            o.saturation  = (o.saturation + 0.15).clamp(-1.0, 1.0);
            o.shadows     = (o.shadows    + 0.05).clamp(-1.0, 1.0);
            o.warm_analog  = true;
        }
        Preset::WarmFilm => {
            // Warm analog film: golden midtones, slight fade, organic feel
            o.exposure    = (o.exposure   + 0.05).clamp(-2.0, 2.0);
            o.saturation  = (o.saturation + 0.10).clamp(-1.0, 1.0);
            o.contrast    = o.contrast * 0.5 - 0.08;
            o.shadows     = (o.shadows    + 0.10).clamp(-1.0, 1.0);
            o.warm_analog  = true;
        }
        Preset::Bw => {
            // Clean neutral B&W
            o.saturation = -1.0;
        }
        Preset::BwContrast => {
            // Dramatic high-contrast B&W
            o.saturation = -1.0;
            o.contrast   = o.contrast * 0.5 + 0.50;
            o.highlights = (o.highlights - 0.05).clamp(-1.0, 1.0);
            o.shadows    = (o.shadows    - 0.05).clamp(-1.0, 1.0);
        }
        Preset::CleanBright => {
            // Airy: boosted exposure, low contrast, subtle vibrance
            o.exposure    = (o.exposure   + 0.28).clamp(-2.0, 2.0);
            o.contrast    = o.contrast * 0.5 - 0.12;
            o.saturation  = (o.saturation + 0.05).clamp(-1.0, 1.0);
            o.vibrance    = (o.vibrance   + 0.15).clamp(-1.0, 1.0);
            o.highlights  = (o.highlights + 0.05).clamp(-1.0, 1.0);
        }
        Preset::SoftGlow => {
            // Portrait-friendly: warm lift, gentle diffusion
            o.exposure    = (o.exposure   + 0.15).clamp(-2.0, 2.0);
            o.contrast    = o.contrast * 0.5 - 0.12;
            o.saturation  = (o.saturation + 0.08).clamp(-1.0, 1.0);
            o.soft_glow   = true;
        }
        Preset::None => { /* identity — handled by early return above */ }
    }
    o
}


fn auto_enhance(pixels: &[u8]) -> (f32, f32, f32) {
    if pixels.is_empty() { return (0.0, 0.0, 0.0); }
    let n = pixels.len() / 4;
    let mut hist = [0u32; 256];
    for chunk in pixels.chunks_exact(4) {
        let l = (0.2126*chunk[0] as f32 + 0.7152*chunk[1] as f32 + 0.0722*chunk[2] as f32) as usize;
        hist[l.min(255)] += 1;
    }
    let p01 = pct_luma(&hist, n, 0.01);
    let p99 = pct_luma(&hist, n, 0.99);
    let mid  = (p01 + p99) / 2.0;
    let exp  = (128.0 / mid.max(1.0)).log2().clamp(-1.5, 1.5);
    let br   = ((128.0 - mid) / 255.0 * 0.3).clamp(-0.3, 0.3);
    let rng  = (p99 - p01).max(1.0);
    let sat  = if rng < 100.0 { 0.20 } else if rng > 220.0 { -0.10 } else { 0.0 };
    (exp, br, sat)
}

fn pct_luma(hist: &[u32; 256], n: usize, pct: f32) -> f32 {
    let target = (n as f32 * pct) as u32;
    let mut acc = 0u32;
    for (i, &c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target { return i as f32; }
    }
    255.0
}

// ── Tonal & colour pixel loop ─────────────────────────────────────────────────

fn tonal_colour(pixels: &[u8], p: &PipelineParams) -> Vec<u8> {
    // Pre-compute per-frame constants outside the hot loop.
    let exposure_mult = 2.0_f32.powf(p.exposure);
    let contrast_f    = if p.contrast >= 0.0 { 1.0 + p.contrast * 1.5 }
                        else                  { 1.0 + p.contrast * 0.75 };

    let mut out = pixels.to_vec();
    let n = out.len() / 4;

    for i in 0..n {
        let b = i * 4;
        let a = out[b + 3];
        let mut r = out[b    ] as f32 / 255.0;
        let mut g = out[b + 1] as f32 / 255.0;
        let mut v = out[b + 2] as f32 / 255.0;

        // ── Tonal adjustments ────────────────────────────────────────────────
        apply_exposure(&mut r, &mut g, &mut v, exposure_mult);
        let l1 = luma(r, g, v);
        apply_highlights(&mut r, &mut g, &mut v, l1, p.highlights);
        apply_shadows(   &mut r, &mut g, &mut v, l1, p.shadows);
        apply_brightness(&mut r, &mut g, &mut v, p.brightness);
        apply_contrast(  &mut r, &mut g, &mut v, contrast_f, p.contrast);

        // ── Colour adjustments ───────────────────────────────────────────────
        let l2 = luma(r, g, v);
        apply_saturation(&mut r, &mut g, &mut v, l2, p.saturation);
        apply_vibrance(  &mut r, &mut g, &mut v, p.vibrance);

        // ── Style grading (preset flags) ─────────────────────────────────────
        if p.bw_warm      { apply_bw_warm(    &mut r, &mut g, &mut v); }
        if p.vintage_fade { apply_vintage_fade(&mut r, &mut g, &mut v); }
        if p.warm_analog  { apply_warm_analog( &mut r, &mut g, &mut v); }
        if p.cool_film    { apply_cool_film(   &mut r, &mut g, &mut v); }
        if p.soft_glow    { apply_soft_glow(   &mut r, &mut g, &mut v); }

        out[b    ] = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
        out[b + 1] = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
        out[b + 2] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        out[b + 3] = a;
    }
    out
}

// ── Per-pixel grading operations (all #[inline]) ──────────────────────────────
//
// Each function encapsulates one adjustment step. Names map directly to the
// slider labels and preset descriptions. All parameters are in [−1, 1] or
// [0, 1] ranges as documented in EditParams.

/// Perceptual luminance using BT.709 coefficients.
#[inline] fn luma(r: f32, g: f32, b: f32) -> f32 { 0.2126*r + 0.7152*g + 0.0722*b }

/// Multiply RGB by an exposure multiplier (2^EV stops, pre-computed once per frame).
#[inline] fn apply_exposure(r: &mut f32, g: &mut f32, v: &mut f32, mult: f32) {
    *r *= mult; *g *= mult; *v *= mult;
}

/// Selectively adjust bright tones without touching shadows.
#[inline] fn apply_highlights(r: &mut f32, g: &mut f32, v: &mut f32, l: f32, amt: f32) {
    if amt.abs() <= 1e-4 { return; }
    let t = smoothstep(0.5, 1.0, l) * amt;
    *r += t*(*r-l)*0.6; *g += t*(*g-l)*0.6; *v += t*(*v-l)*0.6;
}

/// Selectively lift or crush shadow tones.
#[inline] fn apply_shadows(r: &mut f32, g: &mut f32, v: &mut f32, l: f32, amt: f32) {
    if amt.abs() <= 1e-4 { return; }
    let t = smoothstep(0.5, 0.0, l) * amt * 0.6;
    *r += t; *g += t; *v += t;
}

/// Linear brightness offset applied after exposure/tonal range adjustments.
#[inline] fn apply_brightness(r: &mut f32, g: &mut f32, v: &mut f32, amt: f32) {
    if amt.abs() <= 1e-4 { return; }
    *r += amt; *g += amt; *v += amt;
}

/// S-curve contrast centred at mid-grey.
#[inline] fn apply_contrast(r: &mut f32, g: &mut f32, v: &mut f32, cf: f32, amt: f32) {
    if amt.abs() <= 1e-4 { return; }
    *r = s_curve(*r, cf); *g = s_curve(*g, cf); *v = s_curve(*v, cf);
}

/// Global saturation: scale chroma relative to current luminance.
#[inline] fn apply_saturation(r: &mut f32, g: &mut f32, v: &mut f32, l: f32, amt: f32) {
    if amt.abs() <= 1e-4 { return; }
    let m = 1.0 + amt;
    *r = l + (*r-l)*m; *g = l + (*g-l)*m; *v = l + (*v-l)*m;
}

/// Smart saturation that boosts dull colours more than already-saturated ones.
#[inline] fn apply_vibrance(r: &mut f32, g: &mut f32, v: &mut f32, amt: f32) {
    if amt.abs() <= 1e-4 { return; }
    let mx  = r.max(*g).max(*v);
    let mn  = r.min(*g).min(*v);
    let cs  = if mx > 1e-5 { (mx - mn) / mx } else { 0.0 };
    let vib = amt * (1.0 - cs);
    let l   = luma(*r, *g, *v);
    *r = l + (*r-l)*(1.0+vib);
    *g = l + (*g-l)*(1.0+vib);
    *v = l + (*v-l)*(1.0+vib);
}

/// B&W with warm sepia tone (silver-gelatin look).
#[inline] fn apply_bw_warm(r: &mut f32, g: &mut f32, v: &mut f32) {
    let grey = luma(*r, *g, *v);
    *r = (grey*1.10).min(1.0); *g = (grey*0.95).min(1.0); *v = (grey*0.82).min(1.0);
}

/// Faded matte: lifted blacks and slightly muted highlights.
#[inline] fn apply_vintage_fade(r: &mut f32, g: &mut f32, v: &mut f32) {
    *r = *r*0.88+0.08; *g = *g*0.88+0.07; *v = *v*0.88+0.06;
}

/// Warm analog film: golden highlights, slight cool in the shadows.
#[inline] fn apply_warm_analog(r: &mut f32, g: &mut f32, v: &mut f32) {
    let l  = luma(*r, *g, *v);
    let wm = smoothstep(0.4, 1.0, l); // highlight mask
    let cl = smoothstep(0.4, 0.0, l); // shadow mask
    *r += wm*0.07 - cl*0.03;
    *g += wm*0.02 - cl*0.01;
    *v -= wm*0.05 - cl*0.06;
}

/// Cool blue film cast.
#[inline] fn apply_cool_film(r: &mut f32, g: &mut f32, v: &mut f32) {
    *r *= 0.94; *g *= 0.98; *v = (*v*1.06).min(1.0);
}

/// Soft glow: brightens highlights with a gentle diffusion effect.
#[inline] fn apply_soft_glow(r: &mut f32, g: &mut f32, v: &mut f32) {
    let glow = smoothstep(0.55, 1.0, luma(*r, *g, *v)) * 0.08;
    *r = (*r+glow).min(1.0); *g = (*g+glow).min(1.0); *v = (*v+glow).min(1.0);
}

// ── Math helpers ──────────────────────────────────────────────────────────────

#[inline] fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x-e0)/(e1-e0)).clamp(0.0,1.0);
    t*t*(3.0-2.0*t)
}
#[inline] fn s_curve(x: f32, f: f32) -> f32 {
    ((x.clamp(0.0,1.0)-0.5)*f+0.5).clamp(0.0,1.0)
}

// ── Geometric transforms ──────────────────────────────────────────────────────

fn do_rotate(px: &[u8], w: u32, h: u32, rot: Rotation) -> (Vec<u8>, u32, u32) {
    match rot {
        Rotation::R0   => (px.to_vec(), w, h),
        Rotation::R90  => rot90(px, w, h, true),
        Rotation::R180 => rot180(px, w, h),
        Rotation::R270 => rot90(px, w, h, false),
    }
}

fn rot90(px: &[u8], w: u32, h: u32, cw: bool) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = (h, w);
    let mut out = vec![0u8; px.len()];
    for y in 0..h { for x in 0..w {
        let s = ((y*w+x)*4) as usize;
        let (nx,ny) = if cw { (h-1-y, x) } else { (y, w-1-x) };
        let d = ((ny*nw+nx)*4) as usize;
        out[d..d+4].copy_from_slice(&px[s..s+4]);
    }}
    (out, nw, nh)
}

fn rot180(px: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let mut out = vec![0u8; px.len()];
    let n = (w*h) as usize;
    for i in 0..n { out[(n-1-i)*4..][..4].copy_from_slice(&px[i*4..][..4]); }
    (out, w, h)
}

fn do_flip(px: &[u8], w: u32, h: u32, fh: bool, fv: bool) -> (Vec<u8>, u32, u32) {
    let mut out = px.to_vec();
    if fh { for y in 0..h as usize {
        let row = y * w as usize;
        for x in 0..w as usize/2 {
            for c in 0..4 { out.swap((row+x)*4+c, (row+w as usize-1-x)*4+c); }
        }
    }}
    if fv { for y in 0..h as usize/2 {
        for x in 0..w as usize {
            for c in 0..4 { out.swap((y*w as usize+x)*4+c, ((h as usize-1-y)*w as usize+x)*4+c); }
        }
    }}
    (out, w, h)
}

// ── Bilinear scale ────────────────────────────────────────────────────────────

fn scale_bilinear(px: &[u8], w: u32, h: u32, scale: f32) -> (Vec<u8>, u32, u32) {
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (nw*nh*4) as usize];
    let inv = 1.0 / scale;
    for ny in 0..nh {
        let sf = ny as f32 * inv;
        let sy0 = sf as u32; let sy1 = (sy0+1).min(h-1);
        let fy  = sf - sy0 as f32;
        for nx in 0..nw {
            let sf = nx as f32 * inv;
            let sx0 = sf as u32; let sx1 = (sx0+1).min(w-1);
            let fx  = sf - sx0 as f32;
            let p00 = samp(px,w,sx0,sy0); let p10 = samp(px,w,sx1,sy0);
            let p01 = samp(px,w,sx0,sy1); let p11 = samp(px,w,sx1,sy1);
            let d   = ((ny*nw+nx)*4) as usize;
            for c in 0..4 {
                let t = p00[c]+(p10[c]-p00[c])*fx;
                let b = p01[c]+(p11[c]-p01[c])*fx;
                out[d+c] = (t+(b-t)*fy).round().clamp(0.0,255.0) as u8;
            }
        }
    }
    (out, nw, nh)
}

#[inline] fn samp(buf: &[u8], w: u32, x: u32, y: u32) -> [f32;4] {
    let i = ((y*w+x)*4) as usize;
    [buf[i] as f32, buf[i+1] as f32, buf[i+2] as f32, buf[i+3] as f32]
}

// ── Unsharp mask ──────────────────────────────────────────────────────────────

fn unsharp_mask(px: &[u8], w: u32, h: u32, amount: f32) -> Vec<u8> {
    let blurred = box_blur_3x3(px, w, h);
    let mut out = px.to_vec();
    for i in 0..(w*h) as usize {
        let b = i*4;
        for c in 0..3 {
            let o = px[b+c] as f32;
            let bl = blurred[b+c] as f32;
            out[b+c] = (o + amount*(o-bl)).clamp(0.0,255.0).round() as u8;
        }
    }
    out
}

fn box_blur_3x3(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; px.len()];
    let wi = w as i32; let hi = h as i32;
    for y in 0..hi { for x in 0..wi {
        let mut s = [0u32;4]; let mut cnt = 0u32;
        for dy in -1i32..=1 { for dx in -1i32..=1 {
            let nx = x+dx; let ny = y+dy;
            if nx>=0 && nx<wi && ny>=0 && ny<hi {
                let i = ((ny*wi+nx)*4) as usize;
                for c in 0..4 { s[c] += px[i+c] as u32; }
                cnt += 1;
            }
        }}
        let base = ((y*wi+x)*4) as usize;
        for c in 0..4 { out[base+c] = (s[c]/cnt) as u8; }
    }}
    out
}

// ── Unit tests ────────────────────────────────────────────────────────────────
//
// Run with:  cargo test
// Run only editor tests:  cargo test editor
// See stdout: cargo test -- --nocapture

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{DecodedImage, Frame};

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a solid-colour test image (RGBA8).
    fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> DecodedImage {
        let pixels: Vec<u8> = (0..w * h)
            .flat_map(|_| [r, g, b, 255])
            .collect();
        DecodedImage {
            frames: vec![Frame { pixels, delay_ms: 0 }],
            width:  w,
            height: h,
        }
    }

    // ── apply_edits: identity pass ────────────────────────────────────────────

    #[test]
    fn identity_preserves_dimensions() {
        let src = solid(64, 48, 128, 64, 32);
        let out = apply_edits(&src, &EditParams::default());
        assert_eq!(out.width,  src.width,  "width must be unchanged");
        assert_eq!(out.height, src.height, "height must be unchanged");
    }

    #[test]
    fn identity_preserves_pixel_count() {
        let src = solid(32, 32, 10, 20, 30);
        let out = apply_edits(&src, &EditParams::default());
        assert_eq!(
            out.frames[0].pixels.len(),
            (src.width * src.height * 4) as usize
        );
    }

    #[test]
    fn identity_on_solid_white_is_near_white() {
        // After a no-op edit, a white image must stay near-white.
        let src = solid(16, 16, 255, 255, 255);
        let out = apply_edits(&src, &EditParams::default());
        let avg: u32 = out.frames[0].pixels
            .chunks(4)
            .map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32)
            .sum::<u32>() / (16 * 16 * 3);
        assert!(avg > 200, "white image after identity should stay bright, got avg={avg}");
    }

    // ── apply_edits: BW preset ─────────────────────────────────────────────────

    #[test]
    fn bw_preset_desaturates() {
        // A vivid red image should become grey after BW preset.
        let src = solid(16, 16, 255, 0, 0);
        let mut p = EditParams::default();
        p.preset = Preset::Bw;
        let out = apply_edits(&src, &p);
        // In a greyscale image R == G == B for every pixel.
        for chunk in out.frames[0].pixels.chunks(4) {
            let (r, g, b) = (chunk[0], chunk[1], chunk[2]);
            let spread = r.abs_diff(g).max(r.abs_diff(b)).max(g.abs_diff(b));
            assert!(spread <= 2, "BW pixel should be grey, got r={r} g={g} b={b}");
        }
    }

    // ── apply_edits: rotation ─────────────────────────────────────────────────

    #[test]
    fn rotate_cw_swaps_dimensions() {
        let src = solid(64, 32, 0, 0, 0); // landscape
        let mut p = EditParams::default();
        p.rotation = Rotation::R90;  // 90° CW
        let out = apply_edits(&src, &p);
        assert_eq!(out.width,  32, "rotated width should be original height");
        assert_eq!(out.height, 64, "rotated height should be original width");
    }

    #[test]
    fn rotate_ccw_swaps_dimensions() {
        let src = solid(80, 20, 0, 0, 0);
        let mut p = EditParams::default();
        p.rotation = Rotation::R270; // 270° CW == 90° CCW
        let out = apply_edits(&src, &p);
        assert_eq!(out.width,  20);
        assert_eq!(out.height, 80);
    }

    #[test]
    fn rotate_180_preserves_dimensions() {
        let src = solid(50, 30, 0, 0, 0);
        let mut p = EditParams::default();
        p.rotation = Rotation::R180; // 180°
        let out = apply_edits(&src, &p);
        assert_eq!(out.width,  50);
        assert_eq!(out.height, 30);
    }

    // ── apply_edits: flip ─────────────────────────────────────────────────────

    #[test]
    fn flip_h_preserves_dimensions() {
        let src = solid(40, 25, 0, 0, 0);
        let mut p = EditParams::default();
        p.flip_h = true;
        let out = apply_edits(&src, &p);
        assert_eq!(out.width,  40);
        assert_eq!(out.height, 25);
    }

    #[test]
    fn flip_v_preserves_dimensions() {
        let src = solid(40, 25, 0, 0, 0);
        let mut p = EditParams::default();
        p.flip_v = true;
        let out = apply_edits(&src, &p);
        assert_eq!(out.width,  40);
        assert_eq!(out.height, 25);
    }

    #[test]
    fn double_flip_h_is_identity() {
        // Flipping horizontally twice should return the same pixels.
        let src = solid(8, 8, 100, 150, 200);
        // Tint first column to break symmetry
        let mut src2 = src.clone();
        for y in 0..8u32 {
            let i = (y * 8 * 4) as usize;
            src2.frames[0].pixels[i] = 10;
        }
        let mut p = EditParams::default();
        p.flip_h = true;
        let once  = apply_edits(&src2, &p);
        let twice = apply_edits(&once, &p);
        assert_eq!(twice.frames[0].pixels, src2.frames[0].pixels);
    }

    // ── EditParams: params_equal ──────────────────────────────────────────────

    #[test]
    fn default_params_are_equal() {
        let a = EditParams::default();
        let b = EditParams::default();
        assert!(crate::params_equal(&a, &b));
    }

    #[test]
    fn changed_exposure_not_equal() {
        let a = EditParams::default();
        let mut b = EditParams::default();
        b.exposure = 0.5;
        assert!(!crate::params_equal(&a, &b));
    }

    #[test]
    fn changed_preset_not_equal() {
        let a = EditParams::default();
        let mut b = EditParams::default();
        b.preset = Preset::Moody;
        assert!(!crate::params_equal(&a, &b));
    }
}
