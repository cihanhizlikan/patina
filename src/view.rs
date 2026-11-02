//! view.rs — How the image sits on the canvas, and the GPU textures that
//! draw it.
//!
//! The zoom is either Fit or a scale in *physical* pixels per image pixel, so
//! 100% is one image pixel on one screen pixel at any display scale, and the
//! image's corner is snapped to the pixel grid so the pixels line up. Layout
//! is plain functions of a canvas rect, tested without a window.

use std::time::Instant;

use eframe::egui_wgpu::{self, wgpu};
use egui::{Pos2, Rect, Vec2};
use rayon::prelude::*;

/// Where Ctrl+scroll and Ctrl+= / Ctrl+- stop, in screen pixels per image
/// pixel. Fit joins them at its own scale.
pub const STOPS: [f32; 10] = [
    0.125,
    0.25,
    1.0 / 3.0,
    0.5,
    2.0 / 3.0,
    1.0,
    2.0,
    3.0,
    4.0,
    8.0,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    Fit,
    /// Physical screen pixels per image pixel: 1 is 100%.
    Scale(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: Zoom,
    /// The image point at the canvas centre, as fractions of its size.
    pub focus: Vec2,
}

impl Default for View {
    fn default() -> Self {
        View {
            zoom: Zoom::Fit,
            focus: Vec2::splat(0.5),
        }
    }
}

/// The scale, in screen pixels per image pixel, that fits `img` to `canvas`.
pub fn fit_scale(canvas: Rect, img: Vec2, ppp: f32) -> f32 {
    (canvas.width() / img.x).min(canvas.height() / img.y) * ppp
}

impl View {
    pub fn is_fit(&self) -> bool {
        self.zoom == Zoom::Fit
    }

    /// Screen pixels per image pixel now.
    pub fn scale(&self, canvas: Rect, img: Vec2, ppp: f32) -> f32 {
        match self.zoom {
            Zoom::Fit => fit_scale(canvas, img, ppp),
            Zoom::Scale(s) => s,
        }
    }

    fn size(&self, canvas: Rect, img: Vec2, ppp: f32) -> Vec2 {
        img * (self.scale(canvas, img, ppp) / ppp)
    }

    /// The image on the canvas: centred on an axis it does not fill, else
    /// placed by `focus` with no empty margin, its corner on a screen pixel.
    pub fn image_rect(&self, canvas: Rect, img: Vec2, ppp: f32) -> Rect {
        let size = self.size(canvas, img, ppp);
        let axis = |lo: f32, extent: f32, len: f32, focus: f32| {
            if len <= extent {
                lo + (extent - len) * 0.5
            } else {
                (lo + extent * 0.5 - focus * len).clamp(lo + extent - len, lo)
            }
        };
        let x = axis(canvas.min.x, canvas.width(), size.x, self.focus.x);
        let y = axis(canvas.min.y, canvas.height(), size.y, self.focus.y);
        let snap = |v: f32| (v * ppp).round() / ppp;
        Rect::from_min_size(Pos2::new(snap(x), snap(y)), size)
    }

    /// Change the zoom, keeping the image point under `anchor` under it.
    pub fn zoom_to(&mut self, zoom: Zoom, canvas: Rect, img: Vec2, ppp: f32, anchor: Pos2) {
        let before = self.image_rect(canvas, img, ppp);
        let at = ((anchor - before.min) / before.size()).clamp(Vec2::ZERO, Vec2::splat(1.0));
        self.zoom = zoom;
        let size = self.size(canvas, img, ppp);
        self.focus = (canvas.center() - (anchor - at * size)) / size;
        self.clamp_focus(canvas, img, ppp);
    }

    /// Move the image with a drag of `delta` points.
    pub fn pan(&mut self, delta: Vec2, canvas: Rect, img: Vec2, ppp: f32) {
        self.focus -= delta / self.size(canvas, img, ppp);
        self.clamp_focus(canvas, img, ppp);
    }

    /// Keep `focus` where the image still covers the canvas, so a drag past
    /// the edge is not stored up and undone before the image moves again.
    fn clamp_focus(&mut self, canvas: Rect, img: Vec2, ppp: f32) {
        let size = self.size(canvas, img, ppp);
        let axis = |focus: f32, extent: f32, len: f32| {
            if len <= extent {
                0.5
            } else {
                let half = extent * 0.5 / len;
                focus.clamp(half, 1.0 - half)
            }
        };
        self.focus = Vec2::new(
            axis(self.focus.x, canvas.width(), size.x),
            axis(self.focus.y, canvas.height(), size.y),
        );
    }

    /// One stop in or out. Fit is a stop at its own scale; nothing goes
    /// below it, or below 100% for an image smaller than the canvas.
    pub fn step(&mut self, zoom_in: bool, canvas: Rect, img: Vec2, ppp: f32, anchor: Pos2) {
        let fit = fit_scale(canvas, img, ppp);
        let floor = fit.min(1.0) * 0.999;
        let mut ladder: Vec<(f32, Zoom)> = STOPS
            .iter()
            .filter(|&&s| s >= floor && (s - fit).abs() > fit * 0.01)
            .map(|&s| (s, Zoom::Scale(s)))
            .collect();
        ladder.push((fit, Zoom::Fit));
        ladder.sort_by(|a, b| a.0.total_cmp(&b.0));
        let now = self.scale(canvas, img, ppp);
        let next = if zoom_in {
            ladder.iter().find(|(s, _)| *s > now * 1.001)
        } else {
            ladder.iter().rev().find(|(s, _)| *s < now * 0.999)
        };
        if let Some(&(_, z)) = next {
            self.zoom_to(z, canvas, img, ppp, anchor);
        }
    }

    /// What the zoom reads as: `Fit`, `100%`.
    pub fn label(&self) -> String {
        match self.zoom {
            Zoom::Fit => "Fit".into(),
            Zoom::Scale(s) => format!("{}%", (s * 100.0).round()),
        }
    }
}

// ── Textures ──────────────────────────────────────────────────────────────────

/// Which resolution a texture holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// For Fit: Lanczos-reduced when the image shows at under half size.
    Fit,
    /// Every pixel, with a mip chain so a zoom between Fit and 100% does not
    /// alias.
    Full,
}

/// A wgpu texture registered with egui; egui never holds the pixels.
pub struct ImageTexture {
    texture: wgpu::Texture,
    /// Linear, trilinear across mips: for anything under 100%.
    smooth: egui::TextureId,
    /// Nearest when magnified (Full only): from 100% up each image pixel is
    /// a crisp square, and float error cannot blend two of them.
    crisp: Option<egui::TextureId>,
    width: u32,
    height: u32,
    mips: u32,
}

impl ImageTexture {
    /// The handle to draw with at `scale` screen pixels per image pixel.
    pub fn id(&self, scale: f32) -> egui::TextureId {
        match self.crisp {
            Some(crisp) if scale >= 0.999 => crisp,
            _ => self.smooth,
        }
    }

    fn free(self, rs: &egui_wgpu::RenderState) {
        let mut r = rs.renderer.write();
        r.free_texture(&self.smooth);
        if let Some(crisp) = self.crisp {
            r.free_texture(&crisp);
        }
    }
}

/// What a texture was made from: the image's generation, its frame, the tier.
pub type Key = (u64, usize, Tier);

/// One texture and what it holds, so it is uploaded only when that changes.
#[derive(Default)]
pub struct Slot {
    pub tex: Option<ImageTexture>,
    key: Option<Key>,
}

impl Slot {
    pub fn holds(&self, key: Key) -> bool {
        self.key == Some(key)
    }

    pub fn free(&mut self, rs: &egui_wgpu::RenderState) {
        self.key = None;
        if let Some(t) = self.tex.take() {
            t.free(rs);
        }
    }

    pub fn upload(
        &mut self,
        rs: &egui_wgpu::RenderState,
        key: Key,
        pixels: &[u8],
        (w, h): (u32, u32),
        canvas: Vec2,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let t = Instant::now();
        let max = rs.device.limits().max_texture_dimension_2d;
        let (base, w, h) = match key.2 {
            Tier::Fit => lanczos_if_needed(pixels, w, h, canvas),
            Tier::Full => (pixels.to_vec(), w, h),
        };
        let (base, w, h) = cap_to(base, w, h, max);
        let mips = match key.2 {
            Tier::Fit => Vec::new(),
            Tier::Full => mip_chain(&base, w, h),
        };
        let levels = 1 + mips.len() as u32;

        let reuse = self
            .tex
            .as_ref()
            .is_some_and(|it| (it.width, it.height, it.mips) == (w, h, levels));
        if !reuse {
            self.free(rs);
            self.tex = Some(create(rs, w, h, levels, key.2));
        }
        let it = self.tex.as_ref().expect("created above");
        write_level(rs, &it.texture, 0, &base, w, h);
        for (i, (px, mw, mh)) in mips.iter().enumerate() {
            write_level(rs, &it.texture, i as u32 + 1, px, *mw, *mh);
        }
        self.key = Some(key);
        let ms = t.elapsed().as_millis();
        if ms > 2 {
            log!(
                "[upload] {:?} {}×{}, {} levels, {}ms",
                key.2,
                w,
                h,
                levels,
                ms
            );
        }
    }
}

fn create(rs: &egui_wgpu::RenderState, w: u32, h: u32, mips: u32, tier: Tier) -> ImageTexture {
    let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("patina_image"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = |mag| wgpu::SamplerDescriptor {
        label: Some("patina_image"),
        mag_filter: mag,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    };
    let mut r = rs.renderer.write();
    let smooth = r.register_native_texture_with_sampler_options(
        &rs.device,
        &view,
        sampler(wgpu::FilterMode::Linear),
    );
    let crisp = (tier == Tier::Full).then(|| {
        r.register_native_texture_with_sampler_options(
            &rs.device,
            &view,
            sampler(wgpu::FilterMode::Nearest),
        )
    });
    ImageTexture {
        texture,
        smooth,
        crisp,
        width: w,
        height: h,
        mips,
    }
}

fn write_level(
    rs: &egui_wgpu::RenderState,
    texture: &wgpu::Texture,
    level: u32,
    px: &[u8],
    w: u32,
    h: u32,
) {
    rs.queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        px,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

/// The levels below `pixels`, each half the one before by a 2×2 box
/// average, down to 1×1. Not Lanczos: these only carry a zoom between Fit
/// and 100%, and must be quick enough to redo for every edit.
pub fn mip_chain(pixels: &[u8], w: u32, h: u32) -> Vec<(Vec<u8>, u32, u32)> {
    let mut out: Vec<(Vec<u8>, u32, u32)> = Vec::new();
    let (mut sw, mut sh) = (w, h);
    while sw > 1 || sh > 1 {
        let src: &[u8] = out.last().map_or(pixels, |l| &l.0);
        let (dw, dh) = ((sw / 2).max(1), (sh / 2).max(1));
        let mut dst = vec![0u8; (dw * dh * 4) as usize];
        dst.par_chunks_mut(dw as usize * 4)
            .enumerate()
            .for_each(|(y, row)| {
                let y = y as u32;
                let rows = [(2 * y).min(sh - 1), (2 * y + 1).min(sh - 1)];
                for x in 0..dw {
                    let cols = [(2 * x).min(sw - 1), (2 * x + 1).min(sw - 1)];
                    for c in 0..4 {
                        let mut sum = 2u32; // rounds to nearest
                        for &r in &rows {
                            for &q in &cols {
                                sum += src[((r * sw + q) * 4) as usize + c] as u32;
                            }
                        }
                        row[x as usize * 4 + c] = (sum / 4) as u8;
                    }
                }
            });
        out.push((dst, dw, dh));
        (sw, sh) = (dw, dh);
    }
    out
}

/// Shrink to the GPU's largest texture. Only an image over its limit — on
/// desktop GPUs 16384 px, after `main` asks for the adapter's own — loses
/// pixel-true 100%.
fn cap_to(px: Vec<u8>, w: u32, h: u32, max: u32) -> (Vec<u8>, u32, u32) {
    if w <= max && h <= max {
        return (px, w, h);
    }
    let k = max as f32 / w.max(h) as f32;
    let (tw, th) = (
        ((w as f32 * k) as u32).max(1),
        ((h as f32 * k) as u32).max(1),
    );
    let Some(buf) = image::RgbaImage::from_raw(w, h, px) else {
        return (Vec::new(), 0, 0);
    };
    ::log::warn!("[upload] {w}×{h} is over the GPU's {max} px; shown at {tw}×{th}");
    let small = image::imageops::resize(&buf, tw, th, image::imageops::FilterType::Triangle);
    (small.into_raw(), tw, th)
}

/// Pre-downsample `pixels` with Lanczos3 when `canvas_sz` would display the
/// image at less than 50% of its native size, to 2× the display size.
///
/// GPU bilinear filtering aliases large images shown small; this removes it.
/// The 50% threshold and 2× margin balance quality against CPU cost.
fn lanczos_if_needed(pixels: &[u8], w: u32, h: u32, canvas_sz: Vec2) -> (Vec<u8>, u32, u32) {
    let fit = (canvas_sz.x / w as f32).min(canvas_sz.y / h as f32);
    if fit >= 0.5 {
        return (pixels.to_vec(), w, h);
    }
    let target_w = ((w as f32 * fit * 2.0) as u32).max(1);
    let target_h = ((h as f32 * fit * 2.0) as u32).max(1);
    let Some(buf) = image::RgbaImage::from_raw(w, h, pixels.to_vec()) else {
        return (pixels.to_vec(), w, h);
    };
    let resized = image::imageops::resize(
        &buf,
        target_w,
        target_h,
        image::imageops::FilterType::Lanczos3,
    );
    let (rw, rh) = resized.dimensions();
    log!("[upload] Lanczos {w}×{h} → {rw}×{rh} (fit={fit:.2})");
    (resized.into_raw(), rw, rh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(1000.0, 600.0))
    }
    const IMG: Vec2 = Vec2::new(4000.0, 3000.0);

    #[test]
    fn fit_fills_one_axis_and_centres_the_other() {
        let r = View::default().image_rect(canvas(), IMG, 1.0);
        assert_eq!(r.height(), 600.0);
        assert_eq!(r.width(), 800.0);
        assert_eq!(r.min, Pos2::new(100.0, 40.0));
    }

    /// 100% on a 150% display: the image is 4000 / 1.5 points wide, so 4000
    /// screen pixels, and its corner sits on a screen pixel.
    #[test]
    fn a_hundred_percent_is_one_image_pixel_per_screen_pixel() {
        let ppp = 1.5;
        let mut v = View::default();
        v.zoom_to(Zoom::Scale(1.0), canvas(), IMG, ppp, canvas().center());
        let r = v.image_rect(canvas(), IMG, ppp);
        assert!(
            (r.width() * ppp - 4000.0).abs() < 1e-2,
            "{}",
            r.width() * ppp
        );
        assert!((r.height() * ppp - 3000.0).abs() < 1e-2);
        for c in [r.min.x, r.min.y] {
            let px = c * ppp;
            assert!((px - px.round()).abs() < 1e-3, "corner at {px} screen px");
        }
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let ppp = 1.0;
        let mut v = View::default();
        let at = Pos2::new(300.0, 200.0);
        let before = v.image_rect(canvas(), IMG, ppp);
        let u = (at - before.min) / before.size();
        v.zoom_to(Zoom::Scale(1.0), canvas(), IMG, ppp, at);
        let after = v.image_rect(canvas(), IMG, ppp);
        let moved = after.min + u * after.size() - at;
        assert!(moved.length() < 1.0, "the point moved {moved:?}");
    }

    #[test]
    fn a_zoomed_image_never_leaves_a_margin() {
        let mut v = View::default();
        v.zoom_to(Zoom::Scale(1.0), canvas(), IMG, 1.0, canvas().center());
        v.pan(Vec2::new(1e6, 1e6), canvas(), IMG, 1.0);
        let r = v.image_rect(canvas(), IMG, 1.0);
        assert_eq!(r.min, canvas().min, "dragged to the top-left corner");
        // And the drag past the edge is not stored: one step back moves it.
        v.pan(Vec2::new(-10.0, 0.0), canvas(), IMG, 1.0);
        assert_eq!(
            v.image_rect(canvas(), IMG, 1.0).min.x,
            canvas().min.x - 10.0
        );
    }

    #[test]
    fn the_ladder_runs_from_fit_through_a_hundred_percent() {
        let (c, at) = (canvas(), canvas().center());
        let mut v = View::default(); // fit = 0.2
        v.step(false, c, IMG, 1.0, at);
        assert!(v.is_fit(), "nothing below Fit");
        let mut seen = Vec::new();
        for _ in 0..12 {
            v.step(true, c, IMG, 1.0, at);
            seen.push(v.label());
        }
        assert_eq!(
            seen,
            [
                "25%", "33%", "50%", "67%", "100%", "200%", "300%", "400%", "800%", "800%", "800%",
                "800%"
            ]
        );
        for _ in 0..12 {
            v.step(false, c, IMG, 1.0, at);
        }
        assert!(v.is_fit(), "and back out to Fit");
    }

    /// An image smaller than the canvas: Fit enlarges it, and 100% is a
    /// step below Fit.
    #[test]
    fn a_small_image_steps_down_to_a_hundred_percent() {
        let small = Vec2::new(200.0, 150.0); // fit = 4
        let (c, at) = (canvas(), canvas().center());
        let mut v = View::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            v.step(false, c, small, 1.0, at);
            seen.push(v.label());
        }
        assert_eq!(seen, ["300%", "200%", "100%", "100%"]);
    }

    #[test]
    fn each_mip_is_the_average_of_four() {
        // 4×2: left half black, right half white, one grey pixel.
        let mut px = [0, 0, 0, 255].repeat(4 * 2);
        for y in 0..2 {
            for x in 2..4 {
                px[(y * 4 + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        px[..4].copy_from_slice(&[100, 100, 100, 255]);
        let mips = mip_chain(&px, 4, 2);
        let dims: Vec<_> = mips.iter().map(|m| (m.1, m.2)).collect();
        assert_eq!(dims, [(2, 1), (1, 1)]);
        assert_eq!(&mips[0].0[..4], &[25, 25, 25, 255], "(100+0+0+0)/4");
        assert_eq!(&mips[0].0[4..8], &[255, 255, 255, 255]);
        assert_eq!(&mips[1].0[..3], &[140, 140, 140], "(25+255)/2");
    }

    /// What the mips add to a zoomed upload of a 24 MP frame. Run with
    /// `cargo test --release -- --ignored mip_chain_cost --nocapture`.
    #[test]
    #[ignore]
    fn mip_chain_cost() {
        let (w, h) = (6000, 4000);
        let px = vec![128u8; (w * h * 4) as usize];
        let t = Instant::now();
        let mips = mip_chain(&px, w, h);
        eprintln!("{} levels in {} ms", mips.len(), t.elapsed().as_millis());
    }

    #[test]
    fn an_image_over_the_gpu_limit_is_shrunk_to_it() {
        let (px, w, h) = cap_to(vec![9u8; 40 * 10 * 4], 40, 10, 16);
        assert_eq!((w, h), (16, 4));
        assert_eq!(px.len(), 16 * 4 * 4);
        let (_, w, h) = cap_to(vec![0u8; 16 * 4 * 4], 16, 4, 16);
        assert_eq!((w, h), (16, 4), "at the limit, untouched");
    }
}
