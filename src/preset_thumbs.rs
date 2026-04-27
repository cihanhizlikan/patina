//! preset_thumbs.rs — embedded SVG thumbnail for preset tiles.
//!
//! A single placeholder SVG is used for all presets until the real
//! per-preset assets are ready.  When you have the final SVGs, add one
//! const per preset (e.g. `pub static WARM_FILM: &[u8] = include_bytes!(...)`)
//! and update `thumb_svg_for_index` below to return the matching slice.

/// The placeholder SVG used for every preset tile.
pub static PLACEHOLDER: &[u8] =
    include_bytes!("assets/preset_placeholder.svg");

/// Returns the SVG bytes to display for preset index `i`.
/// Once real per-preset SVGs exist, match on `i` here.
pub fn thumb_svg_for_index(_i: usize) -> &'static [u8] {
    PLACEHOLDER
}
