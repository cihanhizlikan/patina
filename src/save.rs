//! save.rs — Writing the edited image to a file, with the source's metadata.
//!
//! The pixels are re-encoded; the EXIF (camera, date, exposure, GPS) and the
//! ICC colour profile are carried over from the file the image was opened
//! from. Patina does not colour-manage, so the edited pixels are still in
//! the source's colour space and its profile still describes them. The EXIF
//! is first made true of the saved pixels (`orientation::for_upright_pixels`).
//!
//! What each format holds, as `image` 0.25 writes it:
//!   JPEG, PNG, WebP — ICC and EXIF.   TIFF — ICC.   BMP, GIF, ICO — neither.
//! Where `image` cannot read the source, this module does: a RAW's EXIF is
//! rebuilt with kamadak-exif, a JPEG XL's comes from its Exif box, and a
//! TIFF's profile from its tag 34675. The saved EXIF's pixel size is the
//! saved image's. XMP embedded in a JPEG, PNG, TIFF or GIF source (read by
//! `image`) is written into a JPEG or PNG by this module after encoding, as
//! `image` has no XMP encoder; a WebP or TIFF output does not carry it.

use std::io::Cursor;
use std::path::Path;

use anyhow::{anyhow, Context};
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat};

use crate::loader::DecodedImage;

/// The metadata a save carries from the source file.
#[derive(Default, Debug)]
pub struct Metadata {
    pub icc: Option<Vec<u8>>,
    /// The TIFF bytes of the EXIF block, already true of upright pixels.
    pub exif: Option<Vec<u8>>,
    /// The XMP packet embedded in the file (keywords, caption, rating,
    /// Lightroom's settings), UTF-8, its orientation already upright.
    pub xmp: Option<Vec<u8>>,
}

impl Metadata {
    /// The ICC profile and EXIF of the image file at `path`. A save in place
    /// reads this before it overwrites the file.
    pub fn read(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        // `image` opens neither RAW nor JPEG XL; RAW files (TIFF-based) are
        // read with kamadak-exif, JPEG XL from its Exif box. Their pixels are
        // rendered to sRGB by the decoders, so no profile is carried.
        let (icc, exif, xmp) = if crate::loader::is_raw_ext(&ext) {
            (None, rebuilt_exif(path), None)
        } else if ext == "jxl" {
            (
                None,
                std::fs::read(path).ok().and_then(|b| jxl_exif(&b)),
                None,
            )
        } else {
            let Ok(mut dec) = decoder(path) else {
                return Self::default();
            };
            let icc = dec.icc_profile().ok().flatten().or_else(|| tiff_icc(path));
            let xmp = dec.xmp_metadata().ok().flatten();
            (icc, dec.exif_metadata().ok().flatten(), xmp)
        };
        let xmp = xmp.and_then(|xmp| upright_xmp(xmp, path));
        let exif = exif.and_then(|mut exif| {
            // An EXIF block that cannot be set upright is dropped, not
            // copied: an Orientation left in it would turn the image twice.
            crate::orientation::for_upright_pixels(&mut exif)
                .inspect_err(|e| log!("[save] EXIF of {} dropped: {e}", path.display()))
                .ok()?;
            Some(exif)
        });
        Self { icc, exif, xmp }
    }
}

/// `xmp` with `tiff:Orientation` set to 1, as the saved pixels are upright.
/// A packet that is not UTF-8 is dropped: it cannot be edited safely, and
/// an orientation left in it would turn the image twice in Lightroom.
fn upright_xmp(xmp: Vec<u8>, path: &Path) -> Option<Vec<u8>> {
    let Ok(mut text) = String::from_utf8(xmp) else {
        log!(
            "[save] embedded XMP of {} dropped: not UTF-8",
            path.display()
        );
        return None;
    };
    if let Some(span) = crate::xmp::value_span(&text, "tiff:Orientation") {
        text.replace_range(span, "1");
    }
    Some(text.into_bytes())
}

/// The XMP properties that say where a picture was taken: the GPS position
/// and the place names IPTC and Photoshop record.
const LOCATION_PROPERTIES: &[&str] = &[
    "exif:GPS",
    "photoshop:City",
    "photoshop:State",
    "photoshop:Country",
    "Iptc4xmpCore:Location",
    "Iptc4xmpCore:CountryCode",
    "Iptc4xmpExt:LocationShown",
    "Iptc4xmpExt:LocationCreated",
];

/// `text` without any property whose name starts with one of
/// `LOCATION_PROPERTIES`, in attribute or element form. `None` when one is
/// still there afterwards, in a shape this does not take apart.
fn xmp_without_location(mut text: String) -> Option<String> {
    for prefix in LOCATION_PROPERTIES {
        let mut from = 0;
        while let Some(found) = text[from..].find(prefix).map(|i| i + from) {
            match property_span(&text, found) {
                Some(span) => {
                    text.replace_range(span.clone(), "");
                    from = span.start;
                }
                None => from = found + prefix.len(),
            }
        }
    }
    let left = LOCATION_PROPERTIES.iter().any(|p| text.contains(p));
    (!left).then_some(text)
}

/// The bytes of the property whose name starts at `at`: ` name="value"` for
/// an attribute, `<name …>…</name>` or `<name …/>` for an element.
fn property_span(text: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let rest = &text[at..];
    let name_len = rest.find(|c: char| !(c.is_alphanumeric() || c == ':' || c == '_'))?;
    let name = &rest[..name_len];
    if text[..at].ends_with('<') {
        let open_end = at + rest.find('>')? + 1;
        if text[..open_end].ends_with("/>") {
            return Some(at - 1..open_end);
        }
        let close = format!("</{name}>");
        let end = open_end + text[open_end..].find(&close)? + close.len();
        return Some(at - 1..end);
    }
    let after = rest[name_len..].strip_prefix('=')?;
    let quote = after.chars().next().filter(|q| *q == '"' || *q == '\'')?;
    let value_end = at + name_len + 2 + after[1..].find(quote)? + 1;
    let start = text[..at].trim_end().len();
    Some(start..value_end)
}

impl Metadata {
    /// The same metadata without the location. An EXIF block whose GPS part
    /// cannot be taken out is dropped whole: a save that was asked to remove
    /// the location must not carry it.
    pub fn without_location(mut self) -> Self {
        if let Some(exif) = &mut self.exif {
            if let Err(e) = crate::orientation::without_location(exif) {
                log!("[save] EXIF dropped, its location could not be removed: {e}");
                self.exif = None;
            }
        }
        self.xmp = self.xmp.take().and_then(|xmp| {
            let text = String::from_utf8(xmp).ok()?;
            let Some(text) = xmp_without_location(text) else {
                log!("[save] embedded XMP dropped, its location could not be removed");
                return None;
            };
            Some(text.into_bytes())
        });
        self
    }
}

/// A RAW file's EXIF, as a fresh block a JPEG, PNG or WebP can carry: the
/// main image's standard tags (camera, lens, date, exposure, GPS) kept; the
/// maker note, whose offsets point into the RAW, private tags and the RAW's
/// own image structure (size, strips, compression, its other IFDs) left out.
/// `None` for a RAW kamadak-exif cannot read (CR3, RAF, …).
fn rebuilt_exif(path: &Path) -> Option<Vec<u8>> {
    use ::exif::{In, Tag};
    let file = std::fs::File::open(path).ok()?;
    let exif = ::exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .inspect_err(|e| log!("[save] no EXIF read from {}: {e}", path.display()))
        .ok()?;
    let structure = |Tag(_, n): Tag| matches!(n, 0x0100..=0x0106 | 0x0115 | 0x0116 | 0x011C);
    let mut writer = ::exif::experimental::Writer::new();
    for f in exif.fields() {
        let Tag(ctx, _) = f.tag;
        let tiff_structure = ctx == ::exif::Context::Tiff && structure(f.tag);
        if f.ifd_num == In::PRIMARY
            && f.tag.description().is_some()
            && f.tag != Tag::MakerNote
            && !tiff_structure
        {
            writer.push_field(f);
        }
    }
    let mut out = Cursor::new(Vec::new());
    writer
        .write(&mut out, exif.little_endian())
        .inspect_err(|e| log!("[save] EXIF of {} not rebuilt: {e}", path.display()))
        .ok()?;
    Some(out.into_inner())
}

/// The EXIF of a JPEG XL file: the TIFF bytes in its `Exif` box, after the
/// box's 4-byte offset to the header. A bare codestream has no boxes, and a
/// Brotli-compressed `brob` box is not read.
fn jxl_exif(file: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = b"\0\0\0\x0CJXL \r\n\x87\n";
    if !file.starts_with(SIGNATURE) {
        return None;
    }
    let mut at = 0usize;
    while at + 8 <= file.len() {
        let size = u32::from_be_bytes(file[at..at + 4].try_into().ok()?) as u64;
        let kind = &file[at + 4..at + 8];
        let (header, size) = match size {
            0 => (8, (file.len() - at) as u64),
            1 => (
                16,
                u64::from_be_bytes(file.get(at + 8..at + 16)?.try_into().ok()?),
            ),
            n => (8, n),
        };
        let end = at.checked_add(usize::try_from(size).ok()?)?;
        let body = file.get(at + header..end)?;
        if kind == b"Exif" {
            let offset = u32::from_be_bytes(body.get(..4)?.try_into().ok()?) as usize;
            return Some(body.get(4 + offset..)?.to_vec());
        }
        at = end;
    }
    None
}

/// A TIFF's ICC profile (tag 34675), which `image` 0.25.10 does not read
/// back. Read only for a file with a TIFF header, so other formats cost no
/// second read.
fn tiff_icc(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 4];
    file.read_exact(&mut head).ok()?;
    if head != *b"II*\0" && head != *b"MM\0*" {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let exif = ::exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    let icc = exif.get_field(
        ::exif::Tag(::exif::Context::Tiff, 0x8773),
        ::exif::In::PRIMARY,
    )?;
    match &icc.value {
        ::exif::Value::Undefined(bytes, _) | ::exif::Value::Byte(bytes) => Some(bytes.clone()),
        _ => None,
    }
}

fn decoder(path: &Path) -> image::ImageResult<impl ImageDecoder> {
    image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()
}

/// Encode frame `frame` of `img` to `path` in the format its extension
/// names, scaled down to fit `max_dim`, with `meta`. Written through a
/// temporary file and a rename, so a failed save leaves the old file whole.
pub fn to_path(
    img: &DecodedImage,
    frame: usize,
    path: &Path,
    quality: u8,
    max_dim: Option<u32>,
    meta: &Metadata,
) -> anyhow::Result<()> {
    let rgba =
        image::RgbaImage::from_raw(img.width, img.height, img.frame(frame).pixels.clone())
            .ok_or_else(|| anyhow!("pixel buffer size mismatch: {}×{}", img.width, img.height))?;
    let mut dynamic = DynamicImage::ImageRgba8(rgba);

    // Resize if a max dimension is specified and the image exceeds it.
    if let Some(max) = max_dim {
        if dynamic.width() > max || dynamic.height() > max {
            dynamic = dynamic.resize(max, max, image::imageops::FilterType::Lanczos3);
        }
    }

    use image::codecs::{jpeg::JpegEncoder, png::PngEncoder, tiff::TiffEncoder, webp::WebPEncoder};
    let (w, h) = (dynamic.width(), dynamic.height());
    let mut out = Vec::new();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("jpg" | "jpeg") => encode(
            JpegEncoder::new_with_quality(&mut out, quality),
            meta,
            &dynamic.to_rgb8(),
            w,
            h,
            ExtendedColorType::Rgb8,
        )?,
        Some("png") => encode(
            PngEncoder::new(&mut out),
            meta,
            &dynamic.to_rgba8(),
            w,
            h,
            ExtendedColorType::Rgba8,
        )?,
        // Lossless by decision, and so `quality` does not apply: lossy would
        // need libwebp (the `webp` crate, 416 KB of C) and a hand-written
        // container to keep EXIF and ICC. See docs/settled-decisions.md.
        Some("webp") => encode(
            WebPEncoder::new_lossless(&mut out),
            meta,
            &dynamic.to_rgba8(),
            w,
            h,
            ExtendedColorType::Rgba8,
        )?,
        Some("tif" | "tiff") => encode(
            TiffEncoder::new(Cursor::new(&mut out)),
            meta,
            &dynamic.to_rgba8(),
            w,
            h,
            ExtendedColorType::Rgba8,
        )?,
        Some("bmp") => dynamic
            .to_rgb8()
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Bmp)?,
        _ => dynamic.write_to(&mut Cursor::new(&mut out), ImageFormat::from_path(path)?)?,
    }
    // `image` writes no XMP; it goes in after encoding, for the formats a
    // camera roll and Lightroom read it from.
    if let Some(xmp) = &meta.xmp {
        out = match ext.as_deref() {
            Some("jpg" | "jpeg") => with_jpeg_xmp(&out, xmp)?,
            Some("png") => with_png_xmp(&out, xmp)?,
            other => {
                log!(
                    "[save] embedded XMP not carried into .{}",
                    other.unwrap_or("")
                );
                out
            }
        };
    }
    crate::xmp::write_atomic(path, &out)
        .with_context(|| format!("could not write {}", path.display()))
}

/// The identifier that opens an XMP APP1 segment.
const XMP_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// `jpeg` with an APP1 segment holding `xmp`, after the leading APPn
/// segments (JFIF, EXIF, ICC). An XMP packet too large for one segment
/// would need Extended XMP, which is not written: it is left out.
fn with_jpeg_xmp(jpeg: &[u8], xmp: &[u8]) -> anyhow::Result<Vec<u8>> {
    let Ok(len) = u16::try_from(2 + XMP_ID.len() + xmp.len()) else {
        log!(
            "[save] embedded XMP of {} bytes left out: over one JPEG segment",
            xmp.len()
        );
        return Ok(jpeg.to_vec());
    };
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return Err(anyhow!("the encoded JPEG has no start marker"));
    }
    let mut at = 2;
    while let Some(&[0xFF, marker, hi, lo]) = jpeg.get(at..at + 4) {
        if !(0xE0..=0xEF).contains(&marker) {
            break;
        }
        at += 2 + u16::from_be_bytes([hi, lo]) as usize;
    }
    let at = at.min(jpeg.len());
    Ok([
        &jpeg[..at],
        &[0xFF, 0xE1],
        &len.to_be_bytes(),
        XMP_ID,
        xmp,
        &jpeg[at..],
    ]
    .concat())
}

/// `png` with an iTXt chunk holding `xmp` under the keyword readers look
/// for, before the first IDAT.
fn with_png_xmp(png: &[u8], xmp: &[u8]) -> anyhow::Result<Vec<u8>> {
    let truncated = || anyhow!("the encoded PNG is malformed");
    let mut at = 8;
    loop {
        let head = png.get(at..at + 8).ok_or_else(truncated)?;
        if &head[4..8] == b"IDAT" {
            break;
        }
        let len = u32::from_be_bytes(head[..4].try_into()?) as usize;
        at += 12 + len;
    }
    // Keyword, NUL, uncompressed, method 0, no language, no translation.
    let data = [&b"XML:com.adobe.xmp\0\0\0\0\0"[..], xmp].concat();
    let len = u32::try_from(data.len())?;
    let typed = [&b"iTXt"[..], &data].concat();
    Ok([
        &png[..at],
        &len.to_be_bytes(),
        &typed,
        &crc32(&typed).to_be_bytes(),
        &png[at..],
    ]
    .concat())
}

/// CRC-32 as PNG chunks use it (ISO 3309), bit by bit: one chunk per save
/// does not need the table.
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn encode(
    mut enc: impl ImageEncoder,
    meta: &Metadata,
    pixels: &[u8],
    w: u32,
    h: u32,
    colour: ExtendedColorType,
) -> image::ImageResult<()> {
    // A format that cannot hold one of them refuses it (TIFF refuses EXIF);
    // the image is saved all the same.
    if let Some(icc) = &meta.icc {
        let _ = enc.set_icc_profile(icc.clone());
    }
    if let Some(exif) = &meta.exif {
        // PixelXDimension/PixelYDimension say the saved size, not the
        // source's (a resize, a 90° turn, a RAW's sensor size).
        let mut exif = exif.clone();
        if let Err(e) = crate::orientation::set_pixel_size(&mut exif, w, h) {
            log!("[save] EXIF pixel size left as it was: {e}");
        }
        let _ = enc.set_exif_metadata(exif);
    }
    enc.write_image(pixels, w, h, colour)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{load_image, LoadOutcome};
    use image::codecs::jpeg::JpegEncoder;
    use std::path::PathBuf;

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("patina-save-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    /// Not a real profile: the save copies it without reading it, so the
    /// test can tell it arrived byte for byte.
    const ICC: &[u8] = b"patina test ICC profile";

    /// Little-endian EXIF as a phone writes it: IFD0 with Orientation 6
    /// (stored landscape, shown portrait) and Software, then IFD1 with the
    /// thumbnail's Compression tag.
    fn phone_exif() -> Vec<u8> {
        let mut t = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        t.extend_from_slice(&[2, 0]); // IFD0 at 8: two entries
        t.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]); // Orientation = 6
        t.extend_from_slice(&[0x31, 0x01, 2, 0, 7, 0, 0, 0, 38, 0, 0, 0]); // Software, 7 bytes at 38
        t.extend_from_slice(&[46, 0, 0, 0]); // IFD1 at 46
        t.extend_from_slice(b"Patina\0\0"); // 38..46, padded
        t.extend_from_slice(&[1, 0]); // IFD1: one entry
        t.extend_from_slice(&[0x03, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]); // Compression = JPEG
        t.extend_from_slice(&[0, 0, 0, 0]);
        t
    }

    /// A 16×8 JPEG, left half red, right half blue, tagged Orientation 6 and
    /// carrying an ICC profile: on screen it is 8×16 with red on top.
    fn phone_jpeg(path: &Path) {
        let img =
            image::RgbImage::from_fn(16, 8, |x, _| image::Rgb(if x < 8 { RED } else { BLUE }));
        let mut out = Vec::new();
        let mut enc = JpegEncoder::new_with_quality(&mut out, 90);
        enc.set_icc_profile(ICC.to_vec()).unwrap();
        enc.set_exif_metadata(phone_exif()).unwrap();
        enc.write_image(img.as_raw(), 16, 8, ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(path, out).unwrap();
    }

    fn loaded(path: &Path) -> DecodedImage {
        let LoadOutcome::Ok(img) = load_image(path) else {
            panic!("{} did not load", path.display())
        };
        img
    }

    /// Width, height and the colour of the top-left and bottom-left corners
    /// as Patina shows the file, so a second turn would show.
    fn as_shown(path: &Path) -> (u32, u32, [u8; 3], [u8; 3]) {
        let img = loaded(path);
        let at = |x: u32, y: u32| {
            let p = &img.frames[0].pixels[((y * img.width + x) * 4) as usize..][..3];
            if p[0] > p[2] {
                RED
            } else {
                BLUE
            }
        };
        (img.width, img.height, at(1, 1), at(1, img.height - 2))
    }

    fn exif_of(path: &Path) -> Option<::exif::Exif> {
        let file = std::fs::File::open(path).unwrap();
        ::exif::Reader::new()
            .read_from_container(&mut std::io::BufReader::new(file))
            .ok()
    }

    fn uint(exif: &::exif::Exif, tag: ::exif::Tag, ifd: ::exif::In) -> Option<u32> {
        exif.get_field(tag, ifd)?.value.get_uint(0)
    }

    fn icc_of(path: &Path) -> Option<Vec<u8>> {
        decoder(path).unwrap().icc_profile().unwrap()
    }

    /// The source as the test builds it, so later assertions are about the
    /// save and not the fixture.
    #[test]
    fn the_fixture_is_a_portrait_phone_photo() {
        let dir = TempDir::new("fixture");
        let src = dir.0.join("phone.jpg");
        phone_jpeg(&src);
        assert_eq!(as_shown(&src), (8, 16, RED, BLUE));
        let exif = exif_of(&src).unwrap();
        assert_eq!(
            uint(&exif, ::exif::Tag::Orientation, ::exif::In::PRIMARY),
            Some(6)
        );
        assert_eq!(
            uint(&exif, ::exif::Tag::Compression, ::exif::In::THUMBNAIL),
            Some(6)
        );
        assert_eq!(icc_of(&src).as_deref(), Some(ICC));
    }

    /// Ctrl+S on an edited JPEG: the file is re-encoded over itself.
    #[test]
    fn saving_a_jpeg_in_place_keeps_its_exif_and_profile_and_stays_upright() {
        let dir = TempDir::new("inplace");
        let path = dir.0.join("phone.jpg");
        phone_jpeg(&path);
        let img = loaded(&path);

        to_path(&img, 0, &path, 95, None, &Metadata::read(&path)).unwrap();

        let exif = exif_of(&path).expect("the EXIF survived");
        let software = exif
            .get_field(::exif::Tag::Software, ::exif::In::PRIMARY)
            .map(|f| f.value.clone());
        assert!(
            matches!(&software, Some(::exif::Value::Ascii(v)) if v[0] == b"Patina"),
            "{software:?}"
        );
        assert_eq!(
            uint(&exif, ::exif::Tag::Orientation, ::exif::In::PRIMARY),
            Some(1),
            "the pixels are turned already"
        );
        assert_eq!(
            uint(&exif, ::exif::Tag::Compression, ::exif::In::THUMBNAIL),
            None,
            "the stale thumbnail is gone"
        );
        assert_eq!(icc_of(&path).as_deref(), Some(ICC));
        assert_eq!(
            as_shown(&path),
            (8, 16, RED, BLUE),
            "shown as before the save, not turned again"
        );
        assert!(!dir.0.join("phone.jpg.tmp").exists());
    }

    /// The latitude 41° 0' 40.00" as three little-endian RATIONALs: a byte
    /// pattern no other part of a test file holds.
    const LATITUDE: [u8; 24] = [
        41, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0xA0, 0x0F, 0, 0, 100, 0, 0, 0,
    ];

    /// Little-endian EXIF with Make "Canon" and a GPS IFD holding
    /// GPSLatitudeRef "N" and GPSLatitude.
    fn located_exif() -> Vec<u8> {
        let mut t = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        t.extend_from_slice(&[3, 0]); // IFD0 at 8: three entries, ends at 50
        t.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); // Orientation = 1
        t.extend_from_slice(&[0x0F, 0x01, 2, 0, 6, 0, 0, 0, 50, 0, 0, 0]); // Make, 6 bytes at 50
        t.extend_from_slice(&[0x25, 0x88, 4, 0, 1, 0, 0, 0, 56, 0, 0, 0]); // GPS IFD at 56
        t.extend_from_slice(&[0, 0, 0, 0]); // no IFD1
        t.extend_from_slice(b"Canon\0"); // 50..56
        t.extend_from_slice(&[2, 0]); // GPS IFD: two entries, ends at 86
        t.extend_from_slice(&[0x01, 0x00, 2, 0, 2, 0, 0, 0, b'N', 0, 0, 0]); // GPSLatitudeRef
        t.extend_from_slice(&[0x02, 0x00, 5, 0, 3, 0, 0, 0, 86, 0, 0, 0]); // GPSLatitude at 86
        t.extend_from_slice(&[0, 0, 0, 0]);
        t.extend_from_slice(&LATITUDE); // 86..110
        t
    }

    /// The Remove location option (set by the Web preset) takes the GPS
    /// out of the saved file — the coordinates' bytes too — and keeps the
    /// rest of the EXIF; without it the location is kept.
    #[test]
    fn remove_location_takes_the_gps_out_of_the_file() {
        let dir = TempDir::new("gps");
        let src = dir.0.join("located.jpg");
        let mut out = Vec::new();
        let mut enc = JpegEncoder::new_with_quality(&mut out, 90);
        enc.set_exif_metadata(located_exif()).unwrap();
        enc.write_image(&[100; 8 * 8 * 3], 8, 8, ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&src, out).unwrap();
        let img = loaded(&src);

        let kept = dir.0.join("kept.jpg");
        to_path(&img, 0, &kept, 90, None, &Metadata::read(&src)).unwrap();
        let exif = exif_of(&kept).unwrap();
        assert!(exif
            .get_field(::exif::Tag::GPSLatitude, ::exif::In::PRIMARY)
            .is_some());

        let shared = dir.0.join("shared.jpg");
        let meta = Metadata::read(&src).without_location();
        to_path(&img, 0, &shared, 90, None, &meta).unwrap();
        let exif = exif_of(&shared).unwrap();
        for tag in [::exif::Tag::GPSLatitude, ::exif::Tag::GPSLatitudeRef] {
            assert!(exif.get_field(tag, ::exif::In::PRIMARY).is_none(), "{tag}");
        }
        assert_eq!(
            exif.get_field(::exif::Tag::Make, ::exif::In::PRIMARY)
                .map(|f| f.display_value().to_string()),
            Some("\"Canon\"".to_string()),
            "the rest of the EXIF stays"
        );
        let bytes = std::fs::read(&shared).unwrap();
        assert!(
            !bytes.windows(LATITUDE.len()).any(|w| w == LATITUDE),
            "the coordinates are not left in the file unreferenced"
        );
    }

    fn field(tag: ::exif::Tag, value: ::exif::Value) -> ::exif::Field {
        ::exif::Field {
            tag,
            ifd_num: ::exif::In::PRIMARY,
            value,
        }
    }

    /// A small PNG to have pixels to save; 16×8.
    fn sixteen_by_eight(dir: &Path) -> DecodedImage {
        let path = dir.join("pixels.png");
        image::RgbaImage::from_pixel(16, 8, image::Rgba([9, 9, 9, 255]))
            .save(&path)
            .unwrap();
        loaded(&path)
    }

    /// A RAW source's EXIF reaches the saved file: camera and exposure kept,
    /// Orientation set to 1 (the decoder turned the pixels), the maker note
    /// and the RAW's own image structure left out, and the pixel size made
    /// the saved one instead of the sensor's.
    #[test]
    fn a_raw_source_s_exif_is_carried_into_a_jpeg() {
        use ::exif::{In, Rational, Tag, Value};
        let dir = TempDir::new("raw-exif");
        let fields = [
            field(Tag::Make, Value::Ascii(vec![b"Canon".to_vec()])),
            field(Tag::Model, Value::Ascii(vec![b"Canon EOS 40D".to_vec()])),
            field(Tag::Orientation, Value::Short(vec![6])),
            field(Tag::ImageWidth, Value::Long(vec![1936])),
            field(
                Tag::ExposureTime,
                Value::Rational(vec![Rational { num: 13, denom: 10 }]),
            ),
            field(
                Tag::MakerNote,
                Value::Undefined(b"canon private".to_vec(), 0),
            ),
            field(Tag::PixelXDimension, Value::Long(vec![3888])),
            field(Tag::PixelYDimension, Value::Long(vec![2592])),
        ];
        let mut w = ::exif::experimental::Writer::new();
        fields.iter().for_each(|f| w.push_field(f));
        let mut tiff = Cursor::new(Vec::new());
        w.write(&mut tiff, true).unwrap();
        let raw = dir.0.join("shot.dng");
        std::fs::write(&raw, tiff.into_inner()).unwrap();

        let dest = dir.0.join("shot.jpg");
        to_path(
            &sixteen_by_eight(&dir.0),
            0,
            &dest,
            90,
            None,
            &Metadata::read(&raw),
        )
        .unwrap();

        let exif = exif_of(&dest).expect("the JPEG has EXIF");
        let shown = |tag| {
            exif.get_field(tag, In::PRIMARY)
                .map(|f| f.display_value().to_string())
        };
        assert_eq!(shown(Tag::Model).as_deref(), Some("\"Canon EOS 40D\""));
        assert_eq!(shown(Tag::ExposureTime).as_deref(), Some("1.3"));
        assert_eq!(
            exif.get_field(Tag::Orientation, In::PRIMARY)
                .and_then(|f| f.value.get_uint(0)),
            Some(1)
        );
        assert_eq!(shown(Tag::MakerNote), None);
        assert_eq!(shown(Tag::ImageWidth), None);
        let px = |tag| {
            exif.get_field(tag, In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
        };
        assert_eq!(
            (px(Tag::PixelXDimension), px(Tag::PixelYDimension)),
            (Some(16), Some(8))
        );
    }

    /// A JPEG XL container's Exif box reaches the saved file.
    #[test]
    fn a_jpeg_xl_source_s_exif_is_carried() {
        let dir = TempDir::new("jxl-exif");
        let boxed = |kind: &[u8], body: &[u8]| {
            [&((body.len() + 8) as u32).to_be_bytes(), kind, body].concat()
        };
        let exif_box = [&[0u8, 0, 0, 0][..], &located_exif()].concat();
        let jxl = [
            &b"\0\0\0\x0CJXL \r\n\x87\n"[..],
            &boxed(b"ftyp", b"jxl \0\0\0\0jxl "),
            &boxed(b"Exif", &exif_box),
            &boxed(b"jxlc", &[0xFF, 0x0A]),
        ]
        .concat();
        let src = dir.0.join("shot.jxl");
        std::fs::write(&src, jxl).unwrap();

        let dest = dir.0.join("shot.png");
        to_path(
            &sixteen_by_eight(&dir.0),
            0,
            &dest,
            90,
            None,
            &Metadata::read(&src),
        )
        .unwrap();

        let exif = decoder(&dest).unwrap().exif_metadata().unwrap().unwrap();
        let exif = ::exif::Reader::new().read_raw(exif).unwrap();
        assert!(exif
            .get_field(::exif::Tag::Make, ::exif::In::PRIMARY)
            .is_some());
    }

    /// A TIFF source's ICC profile, which `image` does not read back, is
    /// read and carried into a PNG.
    #[test]
    fn a_tiff_source_s_profile_is_carried() {
        use image::codecs::tiff::TiffEncoder;
        let dir = TempDir::new("tiff-icc");
        let src = dir.0.join("scan.tif");
        let mut out = Vec::new();
        let mut enc = TiffEncoder::new(Cursor::new(&mut out));
        enc.set_icc_profile(ICC.to_vec()).unwrap();
        enc.write_image(&[50; 16 * 8 * 4], 16, 8, ExtendedColorType::Rgba8)
            .unwrap();
        std::fs::write(&src, out).unwrap();

        let meta = Metadata::read(&src);
        assert_eq!(meta.icc.as_deref(), Some(ICC));
        let dest = dir.0.join("scan.png");
        to_path(&loaded(&src), 0, &dest, 90, None, &meta).unwrap();
        assert_eq!(icc_of(&dest).as_deref(), Some(ICC));
    }

    /// A WebP save keeps every pixel, whatever the quality field says: the
    /// file decodes to exactly the image that was saved.
    #[test]
    fn webp_is_written_lossless() {
        let dir = TempDir::new("webp-lossless");
        let src = dir.0.join("noise.png");
        let mut seed = 0x2545_f491_u32;
        let noise = image::RgbaImage::from_fn(64, 48, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [r, g, b, _] = seed.to_le_bytes();
            image::Rgba([r, g, b, 255])
        });
        noise.save(&src).unwrap();
        let img = loaded(&src);

        let dest = dir.0.join("noise.webp");
        to_path(&img, 0, &dest, 10, None, &Metadata::read(&src)).unwrap();

        let back = image::open(&dest).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (64, 48));
        assert!(back == noise, "every pixel survived quality 10");
    }

    /// Save As to each format that can hold metadata.
    #[test]
    fn save_as_png_and_webp_carry_exif_and_profile_and_tiff_the_profile() {
        let dir = TempDir::new("formats");
        let src = dir.0.join("phone.jpg");
        phone_jpeg(&src);
        let img = loaded(&src);
        let meta = Metadata::read(&src);

        for (name, has_exif) in [("out.png", true), ("out.webp", true), ("out.tif", false)] {
            let dest = dir.0.join(name);
            to_path(&img, 0, &dest, 95, None, &meta).unwrap();

            if has_exif {
                assert_eq!(icc_of(&dest).as_deref(), Some(ICC), "{name}");
            } else {
                // `image` writes a TIFF's profile but does not read one back,
                // so look for it in the file: a TIFF tag stores it as is.
                let bytes = std::fs::read(&dest).unwrap();
                assert!(
                    bytes.windows(ICC.len()).any(|w| w == ICC),
                    "{name}: the profile is in the file"
                );
            }
            let exif = decoder(&dest).unwrap().exif_metadata().unwrap();
            assert_eq!(exif.is_some(), has_exif, "{name}");
            if let Some(exif) = exif {
                assert_eq!(
                    image::metadata::Orientation::from_exif_chunk(&exif),
                    Some(image::metadata::Orientation::NoTransforms),
                    "{name}"
                );
            }
            assert_eq!(as_shown(&dest), (8, 16, RED, BLUE), "{name}");
        }
    }

    #[test]
    fn a_resized_export_keeps_the_metadata() {
        let dir = TempDir::new("resize");
        let src = dir.0.join("phone.jpg");
        let dest = dir.0.join("web.jpg");
        phone_jpeg(&src);

        to_path(&loaded(&src), 0, &dest, 85, Some(8), &Metadata::read(&src)).unwrap();

        assert_eq!(as_shown(&dest), (4, 8, RED, BLUE));
        assert!(exif_of(&dest).is_some());
        assert_eq!(icc_of(&dest).as_deref(), Some(ICC));
    }

    #[test]
    fn a_source_without_metadata_saves_without_it() {
        let dir = TempDir::new("bare");
        let src = dir.0.join("bare.png");
        let dest = dir.0.join("bare.jpg");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]))
            .save(&src)
            .unwrap();

        let meta = Metadata::read(&src);
        assert!(meta.icc.is_none() && meta.exif.is_none(), "{meta:?}");
        to_path(&loaded(&src), 0, &dest, 95, None, &meta).unwrap();

        assert!(exif_of(&dest).is_none());
        assert_eq!(icc_of(&dest), None);
    }

    /// A save that cannot encode leaves the file it would replace whole.
    #[test]
    fn a_failed_save_leaves_the_old_file() {
        let dir = TempDir::new("fail");
        let path = dir.0.join("photo.jpg");
        phone_jpeg(&path);
        let before = std::fs::read(&path).unwrap();
        let wrong = DecodedImage {
            width: 99,
            ..loaded(&path)
        }; // buffer no longer fits

        assert!(to_path(&wrong, 0, &path, 95, None, &Metadata::read(&path)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// An XMP packet as Lightroom embeds it: a keyword, a caption, a rating,
    /// the orientation of the unturned pixels, and where it was taken.
    const EMBEDDED_XMP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
   xmp:Rating="4"
   tiff:Orientation="6"
   exif:GPSLatitude="41,0.666667N"
   photoshop:City="Istanbul">
   <exif:GPSLongitude>28,58.5E</exif:GPSLongitude>
   <dc:subject><rdf:Bag><rdf:li>galata</rdf:li></rdf:Bag></dc:subject>
   <dc:description><rdf:Alt><rdf:li xml:lang="x-default">From the bridge</rdf:li></rdf:Alt></dc:description>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    /// The XMP the image decoder reads back from `path`, as text.
    fn xmp_of(path: &Path) -> Option<String> {
        let xmp = decoder(path).unwrap().xmp_metadata().unwrap()?;
        Some(String::from_utf8(xmp).unwrap())
    }

    /// A JPEG source with `EMBEDDED_XMP` in an APP1 segment.
    fn tagged_jpeg(path: &Path) {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 90)
            .write_image(&[100; 8 * 8 * 3], 8, 8, ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(path, with_jpeg_xmp(&out, EMBEDDED_XMP.as_bytes()).unwrap()).unwrap();
    }

    /// The XMP embedded in a source reaches a saved JPEG and PNG, where the
    /// image decoder (zune-jpeg, the png crate with its CRC check) finds it:
    /// keyword, caption and rating kept, the orientation made upright.
    #[test]
    fn embedded_xmp_is_carried_into_jpeg_and_png() {
        let dir = TempDir::new("xmp");
        let src = dir.0.join("tagged.jpg");
        tagged_jpeg(&src);
        let img = loaded(&src);
        let meta = Metadata::read(&src);

        for name in ["saved.jpg", "saved.png"] {
            let dest = dir.0.join(name);
            to_path(&img, 0, &dest, 90, None, &meta).unwrap();
            let xmp = xmp_of(&dest).unwrap_or_else(|| panic!("{name}: no XMP"));
            assert!(xmp.contains("<rdf:li>galata</rdf:li>"), "{name}");
            assert!(xmp.contains("From the bridge"), "{name}");
            assert!(xmp.contains(r#"xmp:Rating="4""#), "{name}");
            assert!(xmp.contains(r#"tiff:Orientation="1""#), "{name}");
            assert!(!xmp.contains(r#"tiff:Orientation="6""#), "{name}");
            assert!(loaded(&dest).width == 8, "{name} still decodes");
        }
    }

    /// Remove location takes the GPS position and the place names out of
    /// the embedded XMP too, and leaves the rest of it.
    #[test]
    fn remove_location_takes_the_place_out_of_the_xmp() {
        let dir = TempDir::new("xmp-gps");
        let src = dir.0.join("tagged.jpg");
        tagged_jpeg(&src);
        let dest = dir.0.join("shared.jpg");
        let meta = Metadata::read(&src).without_location();
        to_path(&loaded(&src), 0, &dest, 90, None, &meta).unwrap();

        let xmp = xmp_of(&dest).unwrap();
        for gone in ["GPS", "41,0.666667N", "28,58.5E", "City", "Istanbul"] {
            assert!(!xmp.contains(gone), "{gone} left in:\n{xmp}");
        }
        assert!(xmp.contains("<rdf:li>galata</rdf:li>"));
        assert!(
            xmp.contains(r#"tiff:Orientation="1">"#),
            "the element stays closed:\n{xmp}"
        );
        let bytes = std::fs::read(&dest).unwrap();
        assert!(!bytes.windows(8).any(|w| w == b"Istanbul"));
    }

    /// A location property in a shape the stripper does not take apart
    /// drops the XMP whole rather than carry the place.
    #[test]
    fn xmp_with_an_unremovable_location_is_dropped() {
        let odd = "<x><exif:GPSLatitude>41N</x>".to_string();
        assert_eq!(xmp_without_location(odd), None);
        let fine = "<x a=\"1\" exif:GPSAltitude='12'/>".to_string();
        assert_eq!(xmp_without_location(fine).as_deref(), Some("<x a=\"1\"/>"));
    }
}
