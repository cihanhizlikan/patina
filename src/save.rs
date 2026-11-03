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
//! XMP embedded in the file is not carried: `image` has no XMP encoder.
//! RAW and JPEG XL sources yield no metadata: `image` does not open them. A
//! TIFF source yields no profile: `image` 0.25.10 writes a TIFF's ICC tag
//! but reads none back, not even its own (measured in the tests).

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
}

impl Metadata {
    /// The ICC profile and EXIF of the image file at `path`. A save in place
    /// reads this before it overwrites the file.
    pub fn read(path: &Path) -> Self {
        let Ok(mut dec) = decoder(path) else {
            return Self::default();
        };
        let icc = dec.icc_profile().ok().flatten();
        let exif = dec.exif_metadata().ok().flatten().and_then(|mut exif| {
            // An EXIF block that cannot be set upright is dropped, not
            // copied: an Orientation left in it would turn the image twice.
            crate::orientation::for_upright_pixels(&mut exif)
                .inspect_err(|e| log!("[save] EXIF of {} dropped: {e}", path.display()))
                .ok()?;
            Some(exif)
        });
        Self { icc, exif }
    }
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
        self
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
    crate::xmp::write_atomic(path, &out)
        .with_context(|| format!("could not write {}", path.display()))
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
        let _ = enc.set_exif_metadata(exif.clone());
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
}
