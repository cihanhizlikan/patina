//! loader.rs — Image decoding for all supported formats.
//!
//! | Format                    | Crate         |
//! |---------------------------|---------------|
//! | BMP/PNG/JPEG/WebP/TIFF/ICO| `image`       |
//! | GIF (animated)            | `image::codecs::gif` |
//! | JPEG XL (.jxl)            | `jxl-oxide`   |
//! | RAW (50+ camera formats)  | `rawler`      |

use std::path::Path;

// ── Error type ────────────────────────────────────────────────────────────────

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ── Public types ──────────────────────────────────────────────────────────────

/// One animation frame — straight RGBA8, row-major.
#[derive(Clone, Debug)]
pub struct Frame {
    pub pixels: Vec<u8>,
    /// Frame duration in milliseconds. 0 for non-animated formats.
    pub delay_ms: u32,
}

/// A decoded image: one or more frames sharing the same canvas dimensions.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub frames: Vec<Frame>,
    pub width: u32,
    pub height: u32,
}

impl DecodedImage {
    /// Return the frame at `idx`. Always valid — callers use modulo arithmetic.
    pub fn frame(&self, idx: usize) -> &Frame {
        debug_assert!(
            idx < self.frames.len(),
            "frame index {idx} out of range ({})",
            self.frames.len()
        );
        &self.frames[idx]
    }

    fn single(pixels: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            frames: vec![Frame {
                pixels,
                delay_ms: 0,
            }],
            width,
            height,
        }
    }
}

/// The outcome of a `load_image` call.
pub enum LoadOutcome {
    /// Decoded successfully using the file's declared extension.
    Ok(DecodedImage),
    /// The declared extension was wrong; decoded by sniffing the file header.
    /// Carries the image and the correct extension (e.g. `"jpg"`).
    WrongExtension(DecodedImage, String),
    /// Not a recognised image format even after header sniffing.
    Unsupported(String),
}

// ── Supported extensions ──────────────────────────────────────────────────────

/// All extensions Patina can open. Used by the directory navigator to filter
/// the file list and by `is_supported` for drag-and-drop validation.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    // Standard raster
    "bmp", "png", "jpg", "jpeg", "gif", "webp", "tiff", "tif", "ico", "jxl",
    // RAW formats — Canon
    "cr2", "cr3", // RAW formats — Nikon
    "nef", "nrw", // RAW formats — Sony
    "arw", "srf", "sr2", // RAW formats — Fujifilm
    "raf", // RAW formats — Olympus / OM Digital
    "orf", // RAW formats — Adobe universal / DNG-based devices
    "dng", // RAW formats — Pentax
    "pef", "ptx", // RAW formats — Panasonic
    "rw2", // RAW formats — Hasselblad
    "3fr", "fff", // RAW formats — Leaf / Phase One
    "mos", // RAW formats — Sigma
    "x3f", // RAW formats — Samsung
    "srw", // RAW formats — Mamiya
    "mef", // RAW formats — Leica
    "rwl",
];

/// Returns `true` when `path` has a supported image extension.
pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Returns `true` when `ext` (lowercase) is a RAW camera format.
pub fn is_raw_ext(ext: &str) -> bool {
    matches!(
        ext,
        "cr2"
            | "cr3"
            | "nef"
            | "nrw"
            | "arw"
            | "srf"
            | "sr2"
            | "raf"
            | "orf"
            | "dng"
            | "pef"
            | "ptx"
            | "rw2"
            | "3fr"
            | "fff"
            | "mos"
            | "x3f"
            | "srw"
            | "mef"
            | "rwl"
    )
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Decode an image file to straight RGBA8.
///
/// Fast path: trust the declared extension. On failure: sniff the file header.
/// Returns `WrongExtension` when the file decodes under a different format,
/// or `Unsupported` when no known format matches.
pub fn load_image(path: &Path) -> LoadOutcome {
    let ext = file_ext(path);

    // RAW formats bypass the magic-byte check — they have no reliable single
    // signature and rawler detects the format internally from the file header.
    if is_raw_ext(&ext) {
        return match decode_raw(path) {
            Ok(img) => LoadOutcome::Ok(img),
            Err(e) => LoadOutcome::Unsupported(format!("RAW decode failed: {e}")),
        };
    }

    if let Ok(img) = decode_by_ext(&ext, path, DecodeStrategy::ByExtension) {
        return LoadOutcome::Ok(img);
    }

    // Fast path failed — sniff the header.
    match sniff_format(path) {
        Some(actual) if actual != ext => {
            match decode_by_ext(&actual, path, DecodeStrategy::ByContent) {
                Ok(img) => LoadOutcome::WrongExtension(img, actual),
                Err(e) => LoadOutcome::Unsupported(e.to_string()),
            }
        }
        Some(_) => {
            // Sniffed the same extension but still failed — corrupt / truncated.
            let desc = if ext.is_empty() {
                "file".into()
            } else {
                format!(".{ext} file")
            };
            LoadOutcome::Unsupported(format!("Failed to decode {desc}"))
        }
        None => {
            // Not a standard raster — try rawler as a last resort.
            // Some RAW files arrive with generic/wrong extensions (.raw, .bin etc.)
            match decode_raw(path) {
                Ok(img) => LoadOutcome::Ok(img),
                Err(_) => LoadOutcome::Unsupported("Not a recognised image format".into()),
            }
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn file_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
enum DecodeStrategy {
    ByExtension,
    ByContent,
}

fn decode_by_ext(
    ext: &str,
    path: &Path,
    strategy: DecodeStrategy,
) -> Result<DecodedImage, BoxError> {
    match ext {
        "jxl" => decode_jxl(path),
        "gif" => decode_gif(path),
        _ => decode_raster(path, strategy),
    }
}

// ── Magic-byte detection ──────────────────────────────────────────────────────

/// JXL codestream signature: `FF 0A`.
const JXL_CODESTREAM_MAGIC: [u8; 2] = [0xFF, 0x0A];
/// JXL ISO BMFF container signature.
const JXL_CONTAINER_MAGIC: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
];

/// Identify a standard raster format from its first bytes.
/// RAW formats are NOT listed here — they have no reliable single signature.
fn magic_to_format(buf: &[u8]) -> Option<image::ImageFormat> {
    use image::ImageFormat;
    if buf.starts_with(&[0xFF, 0xD8]) {
        return Some(ImageFormat::Jpeg);
    }
    if buf.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some(ImageFormat::Png);
    }
    if buf.starts_with(&[0x42, 0x4D]) {
        return Some(ImageFormat::Bmp);
    }
    if buf.starts_with(b"GIF8") {
        return Some(ImageFormat::Gif);
    }
    if buf.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || buf.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return Some(ImageFormat::Tiff);
    }
    if buf.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some(ImageFormat::Ico);
    }
    if buf.len() >= 12 && buf.starts_with(b"RIFF") && &buf[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    None
}

fn format_to_ext(fmt: image::ImageFormat) -> Option<&'static str> {
    use image::ImageFormat;
    match fmt {
        ImageFormat::Png => Some("png"),
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Bmp => Some("bmp"),
        ImageFormat::Gif => Some("gif"),
        ImageFormat::WebP => Some("webp"),
        ImageFormat::Tiff => Some("tiff"),
        ImageFormat::Ico => Some("ico"),
        _ => None,
    }
}

fn sniff_format(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = [0u8; 32];
    let n = std::fs::File::open(path).ok()?.read(&mut buf).ok()?;
    let buf = &buf[..n];
    if let Some(ext) = magic_to_format(buf).and_then(format_to_ext) {
        return Some(ext.to_owned());
    }
    if buf.starts_with(&JXL_CODESTREAM_MAGIC) || buf.starts_with(&JXL_CONTAINER_MAGIC) {
        return Some("jxl".to_owned());
    }
    None
}

fn read_magic(path: &Path) -> Result<Option<image::ImageFormat>, BoxError> {
    use std::io::Read;
    let mut buf = [0u8; 12];
    let n = std::fs::File::open(path)?.read(&mut buf)?;
    Ok(magic_to_format(&buf[..n]))
}

// ── Decoders ──────────────────────────────────────────────────────────────────

fn decode_raster(path: &Path, strategy: DecodeStrategy) -> Result<DecodedImage, BoxError> {
    use image::ImageDecoder;
    let reader = match strategy {
        DecodeStrategy::ByExtension => {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let declared = image::ImageFormat::from_extension(ext)
                .ok_or_else(|| format!("unsupported extension: {ext}"))?;
            let actual = read_magic(path)?;
            if actual != Some(declared) {
                return Err(format!("magic bytes do not match extension .{ext}").into());
            }
            image::ImageReader::open(path)?
        }
        DecodeStrategy::ByContent => {
            let file = std::fs::File::open(path)?;
            image::ImageReader::new(std::io::BufReader::new(file)).with_guessed_format()?
        }
    };
    // Show the image the way it was shot: the EXIF Orientation tag says how
    // the stored pixels must be turned, and Patina saves rotations there.
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(DecodedImage::single(rgba.into_raw(), w, h))
}

fn decode_gif(path: &Path) -> Result<DecodedImage, BoxError> {
    use image::codecs::gif::GifDecoder;
    use image::{AnimationDecoder, ImageDecoder};
    let file = std::fs::File::open(path)?;
    let decoder = GifDecoder::new(std::io::BufReader::new(file))?;
    let (width, height) = decoder.dimensions();
    let frames: Vec<Frame> = decoder
        .into_frames()
        .filter_map(|f| f.ok())
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            let delay_ms = n.checked_div(d).map_or(100, |ms| ms.max(20));
            Frame {
                pixels: f.into_buffer().into_raw(),
                delay_ms,
            }
        })
        .collect();
    if frames.is_empty() {
        return Err("GIF has no decodable frames".into());
    }
    Ok(DecodedImage {
        frames,
        width,
        height,
    })
}

fn decode_jxl(path: &Path) -> Result<DecodedImage, BoxError> {
    use jxl_oxide::JxlImage;
    let file = std::fs::File::open(path)?;
    let mut image = JxlImage::builder().read(std::io::BufReader::new(file))?;
    image.request_color_encoding(jxl_oxide::EnumColourEncoding::srgb(
        jxl_oxide::RenderingIntent::Relative,
    ));
    let render = image.render_frame(0)?;
    let mut stream = render.stream();
    let width = stream.width() as usize;
    let height = stream.height() as usize;
    let channels = stream.channels() as usize;
    const MAX_DIM: usize = 32_768;
    if width > MAX_DIM || height > MAX_DIM {
        return Err(format!("JXL too large ({width}×{height}); max {MAX_DIM}").into());
    }
    let pixel_count = width.checked_mul(height).ok_or("JXL dimensions overflow")?;
    let mut samples = vec![0f32; pixel_count * channels];
    stream.write_to_buffer(&mut samples);
    Ok(DecodedImage::single(
        jxl_samples_to_rgba8(&samples, pixel_count, channels),
        width as u32,
        height as u32,
    ))
}

/// Decode any supported RAW camera file to RGBA8 via rawler.
///
/// rawler handles all the listed formats internally — we don't need to dispatch
/// by extension. It demosaics the sensor data, applies the camera's embedded
/// white balance and colour matrix, and outputs a standard RGB image.
fn decode_raw(path: &Path) -> Result<DecodedImage, BoxError> {
    use rawler::RawFile;

    // RawFile::new takes a path and a reader
    let file = std::fs::File::open(path)?;
    let bufreader = std::io::BufReader::new(file);
    let mut raw_file = RawFile::new(path, bufreader);

    let decoder = rawler::get_decoder(&mut raw_file)
        .map_err(|e| format!("RAW: no decoder for {}: {e:?}", path.display()))?;

    // full_image(file) → Result<Option<DynamicImage>>
    let img_opt = decoder
        .full_image(&mut raw_file)
        .map_err(|e| format!("RAW: decode failed: {e:?}"))?;

    let img = img_opt.ok_or("RAW: decoder returned no image")?;

    // The full image is the camera's embedded preview, stored the way the
    // sensor lay; the RAW's own Orientation tag says how to turn it, as for a
    // JPEG. Unreadable metadata shows it as stored rather than failing.
    let orientation = decoder
        .raw_metadata(&mut raw_file, rawler::decoders::RawDecodeParams::default())
        .ok()
        .and_then(|m| m.exif.orientation)
        .and_then(|o| u8::try_from(o).ok())
        .and_then(image::metadata::Orientation::from_exif)
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    // rawler builds on `image` 0.24, so its buffer crosses as raw bytes.
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    oriented(rgba.into_raw(), w, h, orientation)
}

/// RGBA8 pixels turned by an EXIF orientation.
fn oriented(
    pixels: Vec<u8>,
    w: u32,
    h: u32,
    orientation: image::metadata::Orientation,
) -> Result<DecodedImage, BoxError> {
    let buf = image::RgbaImage::from_raw(w, h, pixels).ok_or("RAW: buffer size mismatch")?;
    let mut img = image::DynamicImage::ImageRgba8(buf);
    img.apply_orientation(orientation);
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(DecodedImage::single(rgba.into_raw(), w, h))
}

/// The size an image is shown at, read from its header without decoding the
/// pixels: a JPEG stored landscape with Orientation 6 is portrait. Sizes the
/// first window. `None` for what `image` cannot read the header of (RAW,
/// JPEG XL), which then opens at the default size.
pub fn shown_dimensions(path: &Path) -> Option<(u32, u32)> {
    use image::metadata::Orientation::*;
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (w, h) = decoder.dimensions();
    match decoder.orientation().unwrap_or(NoTransforms) {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => Some((h, w)),
        _ => Some((w, h)),
    }
}

// ── JXL helpers ───────────────────────────────────────────────────────────────

fn f32_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn jxl_samples_to_rgba8(samples: &[f32], pixel_count: usize, channels: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; pixel_count * 4];
    for i in 0..pixel_count {
        let s = &samples[i * channels..][..channels];
        let d = &mut rgba[i * 4..][..4];
        let (r, g, b, a) = match channels {
            1 => (s[0], s[0], s[0], 1.0),
            2 => (s[0], s[0], s[0], s[1]),
            3 => (s[0], s[1], s[2], 1.0),
            _ => (s[0], s[1], s[2], s[3]),
        };
        d[0] = f32_to_u8(r);
        d[1] = f32_to_u8(g);
        d[2] = f32_to_u8(b);
        d[3] = f32_to_u8(a);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::metadata::Orientation;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A RAW's preview turned by its Orientation tag: 2×1 red|blue,
    /// Orientation 6, is shown 1×2 with red on top.
    #[test]
    fn a_raw_preview_is_turned_by_its_orientation() {
        let pixels = [RED, BLUE].concat();
        let img = oriented(pixels, 2, 1, Orientation::Rotate90).unwrap();
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(img.frames[0].pixels, [RED, BLUE].concat());

        let img = oriented([RED, BLUE].concat(), 2, 1, Orientation::Rotate270).unwrap();
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(img.frames[0].pixels, [BLUE, RED].concat());
    }

    /// A 16×8 JPEG with the given EXIF Orientation, or none.
    fn jpeg(path: &Path, orientation: Option<u16>) {
        use image::codecs::jpeg::JpegEncoder;
        use image::ImageEncoder;
        let mut enc_out = Vec::new();
        let mut enc = JpegEncoder::new_with_quality(&mut enc_out, 90);
        if let Some(o) = orientation {
            let mut t = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
            t.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0]);
            t.extend_from_slice(&o.to_le_bytes());
            t.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
            enc.set_exif_metadata(t).unwrap();
        }
        enc.write_image(&[128; 16 * 8 * 3], 16, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(path, enc_out).unwrap();
    }

    #[test]
    fn the_first_window_is_sized_as_the_photo_is_shown() {
        let dir = std::env::temp_dir().join(format!("patina-shown-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [(None, (16, 8)), (Some(1), (16, 8)), (Some(3), (16, 8))];
        let turned = [(Some(6), (8, 16)), (Some(8), (8, 16)), (Some(5), (8, 16))];
        for (i, (orientation, want)) in cases.into_iter().chain(turned).enumerate() {
            let path = dir.join(format!("{i}.jpg"));
            jpeg(&path, orientation);
            assert_eq!(shown_dimensions(&path), Some(want), "{orientation:?}");
            let LoadOutcome::Ok(img) = load_image(&path) else {
                panic!("{orientation:?} did not load")
            };
            assert_eq!((img.width, img.height), want, "decoded as shown too");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
