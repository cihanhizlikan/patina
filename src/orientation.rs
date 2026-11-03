//! orientation.rs — Lossless JPEG rotation through the EXIF Orientation tag.
//!
//! A rotate or flip of a JPEG is saved by changing the Orientation tag
//! (0x0112 in IFD0), never by re-encoding: the compressed image data and
//! every other byte of the file stay as they were. The loader applies the
//! tag (`loader::decode_raster`), as do cameras, browsers, Explorer,
//! Photos and Lightroom.
//!
//! Three cases, each keeping the rest of the file byte-for-byte:
//!   1. IFD0 has an Orientation entry — its two value bytes are patched.
//!   2. EXIF exists without one — a copy of IFD0 with the entry added is
//!      appended to the TIFF block and the header points at it. Nothing
//!      moves, so every offset in the block (sub-IFDs, MakerNotes) holds.
//!   3. No EXIF — a minimal APP1 holding only the Orientation is inserted
//!      after SOI and any APP0 (JFIF) segments.
//!
//! `for_upright_pixels` serves the other kind of save, which re-encodes
//! pixels that are already turned (`save.rs`): it sets the tag back to 1.

use std::ops::Range;
use std::path::Path;

use anyhow::{anyhow, bail, Context};

use crate::editor::{self, EditParams};

const ORIENTATION: u16 = 0x0112;
const SHORT: u16 = 3;
const EXIF_ID: &[u8] = b"Exif\0\0";

// ── Composition ───────────────────────────────────────────────────────────────

/// The EXIF orientation that shows the stored pixels the way Patina showed
/// them after applying `p` to an image stored with orientation `existing`.
///
/// Found by search, not by a table: the eight orientations are tried on a
/// 3×2 probe through `image`'s `apply_orientation` (what the loader uses)
/// and compared with `editor::apply_rotation_flip_raw` (what the editor
/// showed). A table would be a third definition of rotation direction to
/// keep in step.
pub fn compose(existing: u8, p: &EditParams) -> u8 {
    let seen = oriented(existing);
    let want = editor::apply_rotation_flip_raw(&seen.0, seen.1, seen.2, p);
    (1..=8)
        .find(|&o| oriented(o) == want)
        .expect("rotations and flips of a rectangle are exactly the eight EXIF orientations")
}

/// The probe as an image stored with EXIF orientation `o` is displayed.
/// Values outside 1–8 are read as 1, as `image` and every viewer do.
fn oriented(o: u8) -> (Vec<u8>, u32, u32) {
    let probe = image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([(y * 3 + x) as u8, 0, 0, 255]));
    let mut img = image::DynamicImage::ImageRgba8(probe);
    img.apply_orientation(
        image::metadata::Orientation::from_exif(o)
            .unwrap_or(image::metadata::Orientation::NoTransforms),
    );
    let (w, h) = (img.width(), img.height());
    (img.into_rgba8().into_raw(), w, h)
}

// ── Save ──────────────────────────────────────────────────────────────────────

/// Save the rotation and flips of `p` to the JPEG at `path` as its EXIF
/// orientation. Does not write when the orientation would not change, and
/// returns whether it wrote.
pub fn save_geometry(path: &Path, p: &EditParams) -> anyhow::Result<bool> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let out = with_geometry(&bytes, p)?;
    if out == bytes {
        return Ok(false);
    }
    crate::xmp::write_atomic(path, &out)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(true)
}

/// `jpeg` with its orientation advanced by `p`.
fn with_geometry(jpeg: &[u8], p: &EditParams) -> anyhow::Result<Vec<u8>> {
    // With no Orientation tag the image reads as orientation 1; adding a
    // tag that only says 1 again would change the file for nothing.
    let fresh = compose(1, p);
    let Some(app1) = find_exif(jpeg)? else {
        return Ok(if fresh == 1 {
            jpeg.to_vec()
        } else {
            insert_app1(jpeg, fresh)
        });
    };
    let tiff = Tiff::parse(&jpeg[app1.tiff.clone()])?;
    match tiff.orientation_value_at()? {
        None if fresh == 1 => Ok(jpeg.to_vec()),
        Some(at) => {
            let existing = tiff.u16(at)? as u8;
            let mut out = jpeg.to_vec();
            let value = tiff.encode_u16(compose(existing, p) as u16);
            out[app1.tiff.start + at..][..2].copy_from_slice(&value);
            Ok(out)
        }
        None => {
            let body = [EXIF_ID, &tiff.with_orientation(fresh)?].concat();
            let len = u16::try_from(body.len() + 2)
                .map_err(|_| anyhow!("the EXIF block is too large to add an orientation to"))?;
            let seg = app1.tiff.start - EXIF_ID.len() - 4;
            Ok([
                &jpeg[..seg],
                &[0xFF, 0xE1],
                &len.to_be_bytes(),
                &body,
                &jpeg[app1.tiff.end..],
            ]
            .concat())
        }
    }
}

// ── Re-encoded saves ──────────────────────────────────────────────────────────

/// Make an EXIF block (its TIFF bytes) true of pixels that are already
/// turned upright, for a save that re-encodes them: Orientation becomes 1,
/// or the saved image would be turned a second time, and IFD1 is cut off,
/// because its thumbnail still shows the image before the edit. Every other
/// byte stays, so offsets into the block hold.
pub fn for_upright_pixels(exif: &mut [u8]) -> anyhow::Result<()> {
    let t = Tiff::parse(exif)?;
    let orientation = t.orientation_value_at()?;
    let next_at = t.ifd0 + 2 + 12 * t.u16(t.ifd0)? as usize;
    t.u32(next_at)?; // in bounds
    let (one, none) = (t.encode_u16(1), t.encode_u32(0));
    if let Some(at) = orientation {
        exif[at..at + 2].copy_from_slice(&one);
    }
    exif[next_at..next_at + 4].copy_from_slice(&none);
    Ok(())
}

/// Take the location out of an EXIF block (its TIFF bytes): IFD0's pointer
/// to the GPS IFD is removed, and the GPS IFD and every value it points to
/// are zeroed, so the coordinates are gone from the bytes, not only from
/// what a reader follows. Other offsets in the block hold. A block without a
/// GPS IFD is left as it is. An error means it could not be done; the caller
/// then drops the EXIF rather than risk the location.
pub fn without_location(exif: &mut [u8]) -> anyhow::Result<()> {
    const GPS_IFD: u16 = 0x8825;
    let t = Tiff::parse(exif)?;
    let count = t.u16(t.ifd0)? as usize;
    let entry = |k: usize| t.ifd0 + 2 + 12 * k;
    let mut found = None;
    for k in 0..count {
        if t.u16(entry(k))? == GPS_IFD {
            found = Some(k);
        }
    }
    let Some(k) = found else {
        return Ok(());
    };

    let gps = t.u32(entry(k) + 8)? as usize;
    let n = t.u16(gps)? as usize;
    // The GPS IFD itself, then each value it keeps out of line.
    let mut erase = Vec::with_capacity(n + 1);
    erase.push(gps..gps + 2 + 12 * n + 4);
    for j in 0..n {
        let e = gps + 2 + 12 * j;
        let size = type_size(t.u16(e + 2)?)? * t.u32(e + 4)? as usize;
        if size > 4 {
            let at = t.u32(e + 8)? as usize;
            erase.push(at..at + size);
        }
    }
    // Everything the edit needs, taken while `t` reads the bytes.
    let (ifd0, at_k, after_k) = (t.ifd0, entry(k), entry(k + 1));
    let ifd0_end = entry(count) + 4; // past the next-IFD offset
    let fewer = t.encode_u16(count as u16 - 1);
    if erase
        .iter()
        .chain([&(ifd0..ifd0_end)])
        .any(|r| r.end > exif.len())
    {
        bail!("the GPS block of the EXIF is truncated");
    }

    for r in erase {
        exif[r].fill(0);
    }
    // Close the gap the pointer leaves: later entries and the next-IFD
    // offset move up one record, the count drops by one.
    exif.copy_within(after_k..ifd0_end, at_k);
    exif[ifd0_end - 12..ifd0_end].fill(0);
    exif[ifd0..ifd0 + 2].copy_from_slice(&fewer);
    Ok(())
}

/// Make the Exif IFD's PixelXDimension and PixelYDimension say `w`×`h`,
/// in place. A SHORT too small for the value becomes a LONG; both fit the
/// entry's 4 value bytes, so nothing moves. Absent fields stay absent.
pub fn set_pixel_size(exif: &mut [u8], w: u32, h: u32) -> anyhow::Result<()> {
    const EXIF_IFD: u16 = 0x8769;
    let t = Tiff::parse(exif)?;
    let mut sub = None;
    for k in 0..t.u16(t.ifd0)? as usize {
        let e = t.ifd0 + 2 + 12 * k;
        if t.u16(e)? == EXIF_IFD {
            sub = Some(t.u32(e + 8)? as usize);
        }
    }
    let Some(sub) = sub else {
        return Ok(());
    };
    let n = t.u16(sub)? as usize;
    let mut writes = Vec::new();
    for j in 0..n {
        let e = sub + 2 + 12 * j;
        let value = match t.u16(e)? {
            0xA002 => w,
            0xA003 => h,
            _ => continue,
        };
        if t.u32(e + 4)? != 1 {
            bail!("PixelDimension holds more than one value");
        }
        let (kind, bytes) = match (t.u16(e + 2)?, u16::try_from(value)) {
            (3, Ok(short)) => (3, [t.encode_u16(short), [0, 0]].concat()),
            (3 | 4, _) => (4, t.encode_u32(value).to_vec()),
            (other, _) => bail!("PixelDimension has field type {other}"),
        };
        writes.push((e, t.encode_u16(kind), bytes));
    }
    for (e, kind, bytes) in writes {
        exif[e + 2..e + 4].copy_from_slice(&kind);
        exif[e + 8..e + 12].copy_from_slice(&bytes);
    }
    Ok(())
}

/// Bytes per value of a TIFF field type.
fn type_size(field_type: u16) -> anyhow::Result<usize> {
    Ok(match field_type {
        1 | 2 | 6 | 7 => 1,   // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => 2,           // SHORT, SSHORT
        4 | 9 | 11 | 13 => 4, // LONG, SLONG, FLOAT, IFD
        5 | 10 | 12 => 8,     // RATIONAL, SRATIONAL, DOUBLE
        other => bail!("unknown TIFF field type {other} in the GPS block"),
    })
}

// ── JPEG segments ─────────────────────────────────────────────────────────────

/// Where the EXIF APP1's TIFF block sits in the file.
struct ExifSegment {
    tiff: Range<usize>,
}

/// The first EXIF APP1 before the image data, if any.
fn find_exif(jpeg: &[u8]) -> anyhow::Result<Option<ExifSegment>> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        bail!("not a JPEG file");
    }
    let mut i = 2;
    loop {
        let (marker, body) = segment(jpeg, i)?;
        match marker {
            0xDA | 0xD9 => return Ok(None), // start of scan / end of image: no more metadata
            0xE1 if jpeg[body.clone()].starts_with(EXIF_ID) => {
                return Ok(Some(ExifSegment {
                    tiff: body.start + EXIF_ID.len()..body.end,
                }));
            }
            _ => i = body.end,
        }
    }
}

/// The marker at `i` and the range of its payload (after the length bytes).
fn segment(jpeg: &[u8], i: usize) -> anyhow::Result<(u8, Range<usize>)> {
    let truncated = || anyhow!("the JPEG file is truncated or malformed");
    let head = jpeg.get(i..i + 4).ok_or_else(truncated)?;
    if head[0] != 0xFF {
        return Err(truncated());
    }
    let len = u16::from_be_bytes([head[2], head[3]]) as usize;
    if len < 2 || i + 2 + len > jpeg.len() {
        return Err(truncated());
    }
    Ok((head[1], i + 4..i + 2 + len))
}

/// `jpeg` with a new APP1 holding only the orientation, after SOI and any
/// APP0 segments (JFIF requires APP0 first).
fn insert_app1(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let mut at = 2;
    while let Ok((0xE0, body)) = segment(jpeg, at) {
        at = body.end;
    }
    let tiff: [u8; 26] = [
        b'M',
        b'M',
        0,
        42,
        0,
        0,
        0,
        8, // big-endian header, IFD0 at 8
        0,
        1, // one entry
        0x01,
        0x12,
        0,
        3,
        0,
        0,
        0,
        1,
        0,
        orientation,
        0,
        0, // Orientation, SHORT, 1
        0,
        0,
        0,
        0, // no IFD1
    ];
    let len = (2 + EXIF_ID.len() + tiff.len()) as u16;
    [
        &jpeg[..at],
        &[0xFF, 0xE1],
        &len.to_be_bytes(),
        EXIF_ID,
        &tiff,
        &jpeg[at..],
    ]
    .concat()
}

// ── TIFF (the EXIF payload) ───────────────────────────────────────────────────

struct Tiff<'a> {
    b: &'a [u8],
    little_endian: bool,
    ifd0: usize,
}

impl<'a> Tiff<'a> {
    fn parse(b: &'a [u8]) -> anyhow::Result<Self> {
        let little_endian = match b.get(..4) {
            Some(b"II*\0") => true,
            Some(b"MM\0*") => false,
            _ => bail!("the EXIF block has no TIFF header"),
        };
        let mut t = Self {
            b,
            little_endian,
            ifd0: 0,
        };
        t.ifd0 = t.u32(4)? as usize;
        Ok(t)
    }

    fn u16(&self, at: usize) -> anyhow::Result<u16> {
        let s: [u8; 2] = self
            .b
            .get(at..at + 2)
            .ok_or_else(|| anyhow!("the EXIF block is truncated"))?
            .try_into()?;
        Ok(if self.little_endian {
            u16::from_le_bytes(s)
        } else {
            u16::from_be_bytes(s)
        })
    }

    fn u32(&self, at: usize) -> anyhow::Result<u32> {
        let s: [u8; 4] = self
            .b
            .get(at..at + 4)
            .ok_or_else(|| anyhow!("the EXIF block is truncated"))?
            .try_into()?;
        Ok(if self.little_endian {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    }

    fn encode_u16(&self, v: u16) -> [u8; 2] {
        if self.little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }

    fn encode_u32(&self, v: u32) -> [u8; 4] {
        if self.little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }

    /// IFD0's entries as raw 12-byte records.
    fn ifd0_entries(&self) -> anyhow::Result<Vec<[u8; 12]>> {
        let count = self.u16(self.ifd0)? as usize;
        (0..count)
            .map(|k| {
                let at = self.ifd0 + 2 + 12 * k;
                Ok(self
                    .b
                    .get(at..at + 12)
                    .ok_or_else(|| anyhow!("the EXIF block is truncated"))?
                    .try_into()?)
            })
            .collect()
    }

    /// Offset of the Orientation entry's value, if IFD0 has one.
    fn orientation_value_at(&self) -> anyhow::Result<Option<usize>> {
        for k in 0..self.u16(self.ifd0)? as usize {
            let at = self.ifd0 + 2 + 12 * k;
            if self.u16(at)? == ORIENTATION {
                if self.u16(at + 2)? != SHORT {
                    bail!("the EXIF orientation has an unexpected type");
                }
                return Ok(Some(at + 8));
            }
        }
        Ok(None)
    }

    /// This block with a copy of IFD0, plus an Orientation entry, appended
    /// and made IFD0. The old IFD0 is left in place, unreferenced.
    fn with_orientation(&self, orientation: u8) -> anyhow::Result<Vec<u8>> {
        let mut entries = self.ifd0_entries()?;
        let next = self.u32(self.ifd0 + 2 + 12 * entries.len())?;
        let mut entry = [0u8; 12];
        entry[0..2].copy_from_slice(&self.encode_u16(ORIENTATION));
        entry[2..4].copy_from_slice(&self.encode_u16(SHORT));
        entry[4..8].copy_from_slice(&self.encode_u32(1));
        entry[8..10].copy_from_slice(&self.encode_u16(orientation as u16));
        // TIFF requires entries in ascending tag order.
        let tag = |e: &[u8; 12]| {
            if self.little_endian {
                u16::from_le_bytes([e[0], e[1]])
            } else {
                u16::from_be_bytes([e[0], e[1]])
            }
        };
        let pos = entries
            .iter()
            .position(|e| tag(e) > ORIENTATION)
            .unwrap_or(entries.len());
        entries.insert(pos, entry);

        let mut out = self.b.to_vec();
        if out.len() % 2 == 1 {
            out.push(0);
        } // IFDs start on a word boundary
        let new_ifd0 = u32::try_from(out.len())?;
        out.extend_from_slice(&self.encode_u16(entries.len() as u16));
        for e in &entries {
            out.extend_from_slice(e);
        }
        out.extend_from_slice(&self.encode_u32(next));
        out[4..8].copy_from_slice(&self.encode_u32(new_ifd0));
        Ok(out)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Rotation;
    use crate::loader::{load_image, LoadOutcome};
    use std::path::PathBuf;

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("patina-orient-{name}-{}", std::process::id()));
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

    /// A 16×8 JPEG, left half red, right half blue: flat colours survive JPEG,
    /// and the halves show which way it was turned.
    fn red_left_jpeg() -> Vec<u8> {
        let img =
            image::RgbImage::from_fn(16, 8, |x, _| image::Rgb(if x < 8 { RED } else { BLUE }));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&img)
            .unwrap();
        out
    }

    fn params(rotation: Rotation, flip_h: bool, flip_v: bool) -> EditParams {
        EditParams {
            rotation,
            flip_h,
            flip_v,
            ..EditParams::default()
        }
    }

    /// Width, height and the colour of each corner (top-left, top-right,
    /// bottom-right, bottom-left), as the loader shows the file. Corners sit
    /// far from the red/blue boundary that JPEG blurs.
    fn as_loaded(path: &Path) -> (u32, u32, [[u8; 3]; 4]) {
        let LoadOutcome::Ok(img) = load_image(path) else {
            panic!("{} did not load", path.display())
        };
        let (w, h) = (img.width, img.height);
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            let p = &img.frames[0].pixels[i..i + 3];
            if p[0] > p[2] {
                RED
            } else {
                BLUE
            }
        };
        (
            w,
            h,
            [at(1, 1), at(w - 2, 1), at(w - 2, h - 2), at(1, h - 2)],
        )
    }

    fn exif_field(path: &Path, tag: ::exif::Tag) -> Option<::exif::Value> {
        let file = std::fs::File::open(path).unwrap();
        let exif = ::exif::Reader::new()
            .read_from_container(&mut std::io::BufReader::new(file))
            .ok()?;
        exif.get_field(tag, ::exif::In::PRIMARY)
            .map(|f| f.value.clone())
    }

    fn exif_orientation(path: &Path) -> Option<u32> {
        exif_field(path, ::exif::Tag::Orientation)?.get_uint(0)
    }

    /// `jpeg` with a little-endian EXIF block holding only Software = "Patina".
    fn with_software_only_exif(jpeg: &[u8]) -> Vec<u8> {
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        tiff.extend_from_slice(&[0x31, 0x01, 2, 0, 7, 0, 0, 0, 26, 0, 0, 0]); // Software, ASCII, 7 at 26
        tiff.extend_from_slice(&[0, 0, 0, 0]); // no IFD1
        tiff.extend_from_slice(b"Patina\0");
        let len = (2 + EXIF_ID.len() + tiff.len()) as u16;
        [
            &jpeg[..2],
            &[0xFF, 0xE1],
            &len.to_be_bytes(),
            EXIF_ID,
            &tiff,
            &jpeg[2..],
        ]
        .concat()
    }

    #[test]
    fn compose_agrees_with_the_editor_for_every_case() {
        // A probe other than compose's own, so a lucky 3×2 match would show.
        let stored =
            image::RgbaImage::from_fn(4, 3, |x, y| image::Rgba([(y * 4 + x) as u8, 7, 0, 255]));
        let view = |o: u8| {
            let mut img = image::DynamicImage::ImageRgba8(stored.clone());
            img.apply_orientation(image::metadata::Orientation::from_exif(o).unwrap());
            (img.width(), img.height(), img.into_rgba8().into_raw())
        };
        for existing in 1..=8 {
            for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
                for (fh, fv) in [(false, false), (true, false), (false, true), (true, true)] {
                    let p = params(rotation, fh, fv);
                    let (w, h, seen) = view(existing);
                    let (edited, ew, eh) = editor::apply_rotation_flip_raw(&seen, w, h, &p);
                    let (sw, sh, saved) = view(compose(existing, &p));
                    assert_eq!(
                        (ew, eh, edited),
                        (sw, sh, saved),
                        "existing {existing}, {p:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rotating_a_jpeg_without_exif_inserts_only_an_orientation() {
        let dir = TempDir::new("noexif");
        let path = dir.0.join("a.jpg");
        let original = red_left_jpeg();
        std::fs::write(&path, &original).unwrap();
        assert_eq!(as_loaded(&path), (16, 8, [RED, BLUE, BLUE, RED]));

        assert!(
            save_geometry(&path, &params(Rotation::R90, false, false)).unwrap(),
            "reported a write"
        );

        // Clockwise: the red left half is now on top.
        assert_eq!(as_loaded(&path), (8, 16, [RED, RED, BLUE, BLUE]));
        assert_eq!(exif_orientation(&path), Some(6));
        let saved = std::fs::read(&path).unwrap();
        let added = saved.len() - original.len();
        let at = saved
            .iter()
            .zip(&original)
            .position(|(a, b)| a != b)
            .unwrap();
        assert_eq!(
            [&saved[..at], &saved[at + added..]].concat(),
            original,
            "only an APP1 was inserted"
        );
    }

    #[test]
    fn rotating_again_changes_two_bytes() {
        let dir = TempDir::new("patch");
        let path = dir.0.join("a.jpg");
        std::fs::write(&path, red_left_jpeg()).unwrap();
        save_geometry(&path, &params(Rotation::R90, false, false)).unwrap();
        let before = std::fs::read(&path).unwrap();

        save_geometry(&path, &params(Rotation::R90, false, false)).unwrap();

        let after = std::fs::read(&path).unwrap();
        assert_eq!(after.len(), before.len());
        assert!(after.iter().zip(&before).filter(|(a, b)| a != b).count() <= 2);
        assert_eq!(exif_orientation(&path), Some(3));
        assert_eq!(as_loaded(&path), (16, 8, [BLUE, RED, RED, BLUE]));
    }

    #[test]
    fn an_exif_block_without_orientation_keeps_its_other_fields() {
        let dir = TempDir::new("software");
        let path = dir.0.join("a.jpg");
        std::fs::write(&path, with_software_only_exif(&red_left_jpeg())).unwrap();

        save_geometry(&path, &params(Rotation::R0, true, false)).unwrap();

        assert_eq!(exif_orientation(&path), Some(2));
        let software = exif_field(&path, ::exif::Tag::Software);
        assert!(
            matches!(&software, Some(::exif::Value::Ascii(v)) if v[0] == b"Patina"),
            "{software:?}"
        );
        assert_eq!(as_loaded(&path), (16, 8, [BLUE, RED, RED, BLUE]));
    }

    #[test]
    fn a_full_turn_leaves_the_file_untouched() {
        let dir = TempDir::new("identity");
        let path = dir.0.join("a.jpg");
        let original = red_left_jpeg();
        std::fs::write(&path, &original).unwrap();

        // Flipping both ways is 180°.
        assert!(
            !save_geometry(&path, &params(Rotation::R180, true, true)).unwrap(),
            "reported no write"
        );

        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    /// What happens after a save: the editor resets, so the next Ctrl+S
    /// carries no rotation. It must neither write nor claim to.
    #[test]
    fn saving_again_with_no_edits_reports_no_write() {
        let dir = TempDir::new("resave");
        let path = dir.0.join("a.jpg");
        std::fs::write(&path, red_left_jpeg()).unwrap();
        assert!(save_geometry(&path, &params(Rotation::R90, false, false)).unwrap());
        let before = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert!(!save_geometry(&path, &EditParams::default()).unwrap());

        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            mtime,
            "the file was not rewritten"
        );
    }

    #[test]
    fn a_file_that_is_not_a_jpeg_is_left_alone() {
        let dir = TempDir::new("garbage");
        let path = dir.0.join("a.jpg");
        std::fs::write(&path, b"not a jpeg").unwrap();

        assert!(save_geometry(&path, &params(Rotation::R90, false, false)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not a jpeg");
        assert!(!dir.0.join("a.jpg.tmp").exists());
    }
}
