//! What the viewer says about itself: the HUD strip, the toast lane, the
//! loading bar, the card for a file that will not open, and the empty state.
//!
//! Timing is plain functions of `Instant`, so it is tested without a window;
//! drawing takes a painter and a rect and owns no state.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Rounding, Stroke, Ui, Vec2};

use crate::ui::{self, tok};
use crate::xmp::{ColourLabel, ImageLabels};

/// The HUD stays this long after the last navigation or mouse move.
pub const HUD_HOLD: Duration = Duration::from_millis(1500);
/// A toast stays this long.
pub const TOAST_HOLD: Duration = Duration::from_millis(2500);
/// Both fade out over this.
pub const FADE: Duration = Duration::from_millis(250);
/// A load quicker than this shows no bar: a flash reads as a glitch.
pub const LOADING_BAR_AFTER: Duration = Duration::from_millis(150);
/// The HUD strip's height.
pub const HUD_H: f32 = 30.0;

/// Opacity of something shown at `since` and held for `hold`: 1 while
/// held, falling to 0 over `FADE`.
pub fn fade_alpha(since: Instant, hold: Duration, now: Instant) -> f32 {
    let t = now.saturating_duration_since(since);
    if t <= hold {
        1.0
    } else {
        1.0 - ((t - hold).as_secs_f32() / FADE.as_secs_f32()).min(1.0)
    }
}

/// When to repaint next for a fade that started at `since`, or `None` once
/// it is over. While it fades, every frame.
pub fn fade_repaint(since: Instant, hold: Duration, now: Instant) -> Option<Duration> {
    let t = now.saturating_duration_since(since);
    if t < hold {
        Some(hold - t)
    } else if t < hold + FADE {
        Some(Duration::ZERO)
    } else {
        None
    }
}

pub fn loading_bar_due(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) >= LOADING_BAR_AFTER
}

/// One toast at a time: a new outcome replaces the one on screen.
#[derive(Default)]
pub struct Toast {
    pub current: Option<(String, Instant)>,
}

impl Toast {
    pub fn show(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::info!("[toast] {text}");
        self.current = Some((text, Instant::now()));
    }
}

/// The words a rating change reads as.
pub fn rating_toast(rating: u8) -> String {
    match rating {
        0 => "Rating cleared".into(),
        n => format!("Rated {}", "\u{2605}".repeat(n as usize)),
    }
}

pub fn label_toast(label: ColourLabel) -> String {
    match label {
        ColourLabel::None => "Label cleared".into(),
        l => format!("Label: {}", l.xmp_name()),
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

fn faded(c: Color32, alpha: f32) -> Color32 {
    c.gamma_multiply(alpha)
}

/// What the HUD shows for the current image.
pub struct HudInfo<'a> {
    pub name: &'a str,
    pub position: Option<(usize, usize)>,
    pub dims: Option<(u32, u32)>,
    pub labels: &'a ImageLabels,
}

/// A strip across the top of the canvas: name, `12 / 129`, pixel size on the
/// left; stars and the colour label on the right.
pub fn draw_hud(p: &Painter, canvas: Rect, info: &HudInfo, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let strip = Rect::from_min_size(canvas.min, Vec2::new(canvas.width(), HUD_H));
    p.rect_filled(strip, Rounding::ZERO, faded(tok::SCRIM, alpha));
    let cy = strip.center().y;
    let mut x = strip.min.x + 14.0;
    let text = |s: &str, font: FontId, c: Color32, x: &mut f32| {
        let r = p.text(
            Pos2::new(*x, cy),
            Align2::LEFT_CENTER,
            s,
            font,
            faded(c, alpha),
        );
        *x = r.max.x + 14.0;
    };
    text(info.name, ui::title_font(ui::BODY), tok::TEXT_1, &mut x);
    if let Some((i, n)) = info.position.filter(|(_, n)| *n > 1) {
        let s = format!("{i} / {n}");
        text(&s, FontId::proportional(ui::BODY), tok::TEXT_2, &mut x);
    }
    if let Some((w, h)) = info.dims {
        let s = format!("{w} \u{00D7} {h}");
        text(&s, FontId::proportional(ui::BODY), tok::TEXT_2, &mut x);
    }

    // Right side, drawn right to left.
    let mut rx = strip.max.x - 14.0;
    if info.labels.colour != ColourLabel::None {
        let c = Pos2::new(rx - 6.0, cy);
        p.circle_filled(c, 6.0, faded(info.labels.colour.color(), alpha));
        p.circle_stroke(c, 6.0, Stroke::new(1.0, faded(tok::ON_PHOTO_RING, alpha)));
        rx -= 12.0 + 12.0;
    }
    for i in 0..info.labels.rating {
        let c = Pos2::new(rx - 7.0 - i as f32 * 17.0, cy);
        crate::draw_star(p, c, 7.0, 2.9, faded(tok::STAR, alpha));
    }
}

/// The toast lane: one pill centred near the bottom of the canvas.
pub fn draw_toast(p: &Painter, canvas: Rect, text: &str, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let galley = p.layout_no_wrap(
        text.to_owned(),
        FontId::proportional(ui::BODY),
        faded(tok::TEXT_1, alpha),
    );
    let size = galley.size() + Vec2::new(32.0, 18.0);
    let rect = Rect::from_center_size(
        Pos2::new(canvas.center().x, canvas.max.y - 32.0 - size.y * 0.5),
        size,
    );
    p.rect(
        rect,
        Rounding::same(size.y * 0.5),
        faded(tok::RAISED, alpha),
        Stroke::new(1.0, faded(tok::BORDER_STRONG, alpha)),
    );
    p.galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
}

/// A label pinned to the canvas's top-left corner while it is true, such as
/// `Original` while Backslash is held: the screen says it is not the edit.
pub fn draw_tag(p: &Painter, canvas: Rect, text: &str) {
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(ui::BODY), tok::TEXT_1);
    let size = galley.size() + Vec2::new(24.0, 12.0);
    let rect = Rect::from_min_size(canvas.min + Vec2::splat(14.0), size);
    p.rect(
        rect,
        Rounding::same(size.y * 0.5),
        tok::RAISED,
        Stroke::new(1.0, tok::BORDER_STRONG),
    );
    p.galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
}

/// An indeterminate 2 px bar along the top of the canvas: a segment that
/// sweeps across every 1.2 s.
pub fn draw_loading_bar(p: &Painter, canvas: Rect, elapsed: Duration) {
    let phase = (elapsed.as_secs_f32() / 1.2).fract();
    let w = canvas.width();
    let seg = w * 0.3;
    let x0 = canvas.min.x - seg + phase * (w + seg);
    let r = Rect::from_min_max(
        Pos2::new(x0.max(canvas.min.x), canvas.min.y),
        Pos2::new((x0 + seg).min(canvas.max.x), canvas.min.y + 2.0),
    );
    p.rect_filled(r, Rounding::ZERO, tok::ACCENT);
}

/// In place of the image when it would not open. Browsing goes on around it.
pub fn draw_bad_file_card(p: &Painter, canvas: Rect, name: &str, reason: &str) {
    let w = (canvas.width() - 48.0).clamp(200.0, 440.0);
    let inner = w - 40.0;
    let title = p.layout(
        format!("Can\u{2019}t open {name}"),
        ui::title_font(ui::TITLE),
        tok::TEXT_1,
        inner,
    );
    let body = p.layout(
        reason.to_owned(),
        FontId::proportional(ui::BODY),
        tok::TEXT_2,
        inner,
    );
    let hint = p.layout(
        "\u{2190} \u{2192} keep browsing  \u{00B7}  Del moves it to the Recycle Bin".to_owned(),
        FontId::proportional(ui::CAPTION),
        tok::TEXT_3,
        inner,
    );
    let h = 20.0 + title.size().y + 8.0 + body.size().y + 14.0 + hint.size().y + 20.0;
    let card = Rect::from_center_size(canvas.center(), Vec2::new(w, h));
    p.rect(
        card,
        Rounding::same(8.0),
        tok::SURFACE,
        Stroke::new(1.0, tok::BORDER),
    );
    let mut y = card.min.y + 20.0;
    let x = card.min.x + 20.0;
    for (g, gap) in [(title, 8.0), (body, 14.0), (hint, 0.0)] {
        let gh = g.size().y;
        p.galley(Pos2::new(x, y), g, Color32::WHITE);
        y += gh + gap;
    }
}

/// Nothing open yet. Returns whether Open… was clicked.
pub fn draw_empty_state(ui: &mut Ui, canvas: Rect) -> bool {
    let mut open = false;
    let block = Rect::from_center_size(canvas.center(), Vec2::new(320.0, 120.0));
    ui.allocate_ui_at_rect(block, |ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Drop an image here")
                    .font(ui::title_font(ui::TITLE))
                    .color(tok::TEXT_1),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("or press Ctrl+O")
                    .size(ui::BODY)
                    .color(tok::TEXT_2),
            );
            ui.add_space(14.0);
            open = ui::Button::new(ui::Kind::Secondary, "Open\u{2026}")
                .show(ui)
                .clicked();
        });
    });
    open
}

/// An outline while files are dragged over the window: it will take them.
pub fn draw_drop_target(p: &Painter, canvas: Rect) {
    p.rect_stroke(
        canvas.shrink(8.0),
        Rounding::same(10.0),
        Stroke::new(2.0, tok::ACCENT_LINE),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_holds_then_falls_to_nothing() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(fade_alpha(t0, HUD_HOLD, at(0)), 1.0);
        assert_eq!(fade_alpha(t0, HUD_HOLD, at(1500)), 1.0);
        let mid = fade_alpha(t0, HUD_HOLD, at(1625));
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
        assert_eq!(fade_alpha(t0, HUD_HOLD, at(1750)), 0.0);
        assert_eq!(fade_alpha(t0, HUD_HOLD, at(9000)), 0.0);
    }

    #[test]
    fn a_fade_asks_for_frames_only_while_it_lasts() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(
            fade_repaint(t0, HUD_HOLD, at(500)),
            Some(Duration::from_millis(1000)),
            "sleeps through the hold"
        );
        assert_eq!(fade_repaint(t0, HUD_HOLD, at(1600)), Some(Duration::ZERO));
        assert_eq!(fade_repaint(t0, HUD_HOLD, at(1800)), None, "then idles");
    }

    #[test]
    fn a_quick_load_shows_no_bar() {
        let t0 = Instant::now();
        assert!(!loading_bar_due(t0, t0 + Duration::from_millis(149)));
        assert!(loading_bar_due(t0, t0 + Duration::from_millis(150)));
    }

    #[test]
    fn outcomes_read_as_words() {
        assert_eq!(rating_toast(0), "Rating cleared");
        assert_eq!(rating_toast(3), "Rated \u{2605}\u{2605}\u{2605}");
        assert_eq!(label_toast(ColourLabel::Green), "Label: Green");
        assert_eq!(label_toast(ColourLabel::None), "Label cleared");
    }
}
