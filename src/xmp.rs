//! xmp.rs — Read and write XMP sidecar files for star ratings and colour labels.
//!
//! XMP sidecars are plain XML files stored alongside the image with a `.xmp`
//! extension. They are the industry-standard way to store non-destructive
//! metadata that works across Lightroom, Capture One, and other tools.
//!
//! We write a minimal XMP packet containing only the fields we manage:
//!   xmp:Rating   (integer 0–5, 0 = unrated)
//!   xmp:Label    (string: "Red", "Yellow", "Green", "Blue", "Purple", or "")

use std::ops::Range;
use std::path::{Path, PathBuf};

// ── Colour label ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColourLabel {
    #[default]
    None,
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl ColourLabel {
    pub fn xmp_name(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Red => "Red",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
        }
    }

    pub fn from_xmp(s: &str) -> Self {
        match s.trim() {
            "Red" => Self::Red,
            "Yellow" => Self::Yellow,
            "Green" => Self::Green,
            "Blue" => Self::Blue,
            "Purple" => Self::Purple,
            _ => Self::None,
        }
    }

    /// egui colour for the label dot in the viewer.
    pub fn color(self) -> egui::Color32 {
        match self {
            Self::None => egui::Color32::TRANSPARENT,
            Self::Red => crate::ui::tok::LABEL_RED,
            Self::Yellow => crate::ui::tok::LABEL_YELLOW,
            Self::Green => crate::ui::tok::LABEL_GREEN,
            Self::Blue => crate::ui::tok::LABEL_BLUE,
            Self::Purple => crate::ui::tok::LABEL_PURPLE,
        }
    }
}

// ── Image labels ──────────────────────────────────────────────────────────────

/// Star rating + colour label for one image. Persisted in an XMP sidecar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageLabels {
    /// 0 = unrated, 1–5 = star rating. Values outside [0,5] are clamped.
    pub rating: u8,
    pub colour: ColourLabel,
}

impl ImageLabels {
    pub fn is_empty(&self) -> bool {
        self.rating == 0 && self.colour == ColourLabel::None
    }
}

// ── Sidecar path ──────────────────────────────────────────────────────────────

/// The XMP sidecar for `image_path`, of the two names in use:
/// `photo.jpg.xmp` (darktable, and what Patina creates) and `photo.xmp`
/// (Lightroom, Capture One). The full-filename one wins when both exist;
/// `photo.xmp` is used when only it exists; with neither, the result is the
/// full-filename path, which is created on the first write.
pub fn sidecar_path(image_path: &Path) -> PathBuf {
    let full = full_name_sidecar(image_path);
    if full.exists() {
        return full;
    }
    let stem = image_path.with_extension("xmp");
    if stem != full && stem.is_file() {
        return stem;
    }
    full
}

/// The sidecars that go to the Recycle Bin with `image_path`: its
/// full-filename one, and a `photo.xmp` when no other file in the folder
/// shares the stem — beside a `photo.jpg`, a RAW's `photo.xmp` is still the
/// JPEG's too.
pub fn sidecars_of(image_path: &Path) -> Vec<PathBuf> {
    let full = full_name_sidecar(image_path);
    let stem_xmp = image_path.with_extension("xmp");
    let mut out = Vec::new();
    if full.is_file() {
        out.push(full.clone());
    }
    if stem_xmp != full && stem_xmp.is_file() && !stem_is_shared(image_path, &stem_xmp) {
        out.push(stem_xmp);
    }
    out
}

/// Whether a file other than `image_path` and `stem_xmp` has the same stem.
/// An unreadable folder counts as shared, so nothing is deleted on a guess.
fn stem_is_shared(image_path: &Path, stem_xmp: &Path) -> bool {
    let stem = image_path.file_stem().unwrap_or_default().to_string_lossy();
    let Some(Ok(entries)) = image_path.parent().map(std::fs::read_dir) else {
        return true;
    };
    entries.flatten().map(|e| e.path()).any(|p| {
        p != image_path
            && p != stem_xmp
            && p.file_stem()
                .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem))
    })
}

/// After `old` was renamed to `new`, take its full-filename sidecar along.
/// A `photo.xmp` stays: the rename keeps the stem (it only corrects the
/// extension), and the file may be a RAW's. An existing sidecar under the
/// new name is never replaced.
pub fn rename_sidecar(old: &Path, new: &Path) -> std::io::Result<()> {
    let from = full_name_sidecar(old);
    if !from.is_file() {
        return Ok(());
    }
    crate::rename_no_replace(&from, &full_name_sidecar(new))
}

/// `photo.jpg` → `photo.jpg.xmp`: unlike `photo.xmp`, it cannot be shared
/// by `photo.jpg` and `photo.png`.
fn full_name_sidecar(image_path: &Path) -> PathBuf {
    let mut name = image_path.file_name().unwrap_or_default().to_os_string();
    name.push(".xmp");
    image_path.with_file_name(name)
}

// ── Read ──────────────────────────────────────────────────────────────────────

/// Read labels from the XMP sidecar for `image_path`.
/// Returns `ImageLabels::default()` if no sidecar exists or parsing fails.
pub fn read_labels(image_path: &Path) -> ImageLabels {
    let xmp_path = sidecar_path(image_path);
    let Ok(text) = std::fs::read_to_string(&xmp_path) else {
        return ImageLabels::default();
    };
    parse_xmp(&text)
}

fn parse_xmp(text: &str) -> ImageLabels {
    let mut labels = ImageLabels::default();

    // Text-search approach — avoids pulling in an XML parser dependency.
    // Handles both attribute style (xmp:Tag="value") and element style (<xmp:Tag>value</xmp:Tag>).
    if let Some(r) = value_span(text, "xmp:Rating") {
        labels.rating = text[r].trim().parse::<u8>().unwrap_or(0).min(5);
    }
    if let Some(l) = value_span(text, "xmp:Label") {
        labels.colour = ColourLabel::from_xmp(text[l].trim());
    }
    labels
}

/// Byte range of a tag's value, in attribute or element syntax.
fn value_span(text: &str, tag: &str) -> Option<Range<usize>> {
    attr_span(text, tag).or_else(|| element_span(text, tag))
}

/// XML allows either quote around an attribute value; `build_xmp_packet`
/// writes single quotes, Lightroom and darktable write double.
fn attr_span(text: &str, tag: &str) -> Option<Range<usize>> {
    ['"', '\''].into_iter().find_map(|quote| {
        let needle = format!("{tag}={quote}");
        let start = text.find(&needle)? + needle.len();
        let end = text[start..].find(quote)? + start;
        Some(start..end)
    })
}

fn element_span(text: &str, tag: &str) -> Option<Range<usize>> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(start..end)
}

// ── Write ─────────────────────────────────────────────────────────────────────

/// Render a minimal XMP packet string for the given rating and label name.
fn build_xmp_packet(rating: u8, label: &str) -> String {
    format!(
        "<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>
         <x:xmpmeta xmlns:x='adobe:ns:meta/'>
             <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
                 <rdf:Description rdf:about=''
                       xmlns:xmp='http://ns.adobe.com/xap/1.0/'
                       xmp:Rating='{rating}'
                       xmp:Label='{label}'/>
             </rdf:RDF>
         </x:xmpmeta>
         <?xpacket end='w'?>
"
    )
}

/// Whether `text` is exactly a packet `build_xmp_packet` wrote, and so holds
/// nothing but our two fields.
fn is_own_packet(text: &str) -> bool {
    let labels = parse_xmp(text);
    text == build_xmp_packet(labels.rating, labels.colour.xmp_name())
}

/// Write labels to the XMP sidecar for `image_path`.
///
/// A sidecar written by another tool (Lightroom, darktable, Capture One)
/// holds develop settings, keywords and history, so it is edited in place:
/// only `xmp:Rating` and `xmp:Label` change, every other byte is kept, and
/// it is never deleted. Only a sidecar exactly as Patina writes it, under the
/// name Patina gives it, is removed when the labels are cleared: a `photo.xmp`
/// may be shared with a RAW of the same name. The write is atomic, so a crash
/// cannot leave a truncated sidecar.
pub fn write_labels(image_path: &Path, labels: &ImageLabels) -> std::io::Result<()> {
    let xmp_path = sidecar_path(image_path);
    let ours = xmp_path == full_name_sidecar(image_path);
    let existing = match std::fs::read_to_string(&xmp_path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };

    match existing {
        None if labels.is_empty() => Ok(()),
        None => write_atomic(
            &xmp_path,
            build_xmp_packet(labels.rating, labels.colour.xmp_name()),
        ),
        Some(text) if ours && labels.is_empty() && is_own_packet(&text) => {
            std::fs::remove_file(&xmp_path)
        }
        Some(text) => write_atomic(&xmp_path, &set_labels(&text, labels)?),
    }
}

/// `text` with the rating and label set, everything else untouched.
fn set_labels(text: &str, labels: &ImageLabels) -> std::io::Result<String> {
    let text = set_value(
        text,
        "xmp:Rating",
        &labels.rating.to_string(),
        labels.rating == 0,
    )?;
    set_value(
        &text,
        "xmp:Label",
        labels.colour.xmp_name(),
        labels.colour == ColourLabel::None,
    )
}

/// Replace a tag's value where it is; otherwise add it as an attribute of the
/// first `rdf:Description`. An absent tag that would only be set to its
/// default (`is_default`) stays absent.
fn set_value(text: &str, tag: &str, value: &str, is_default: bool) -> std::io::Result<String> {
    if let Some(span) = value_span(text, tag) {
        let mut out = text.to_owned();
        out.replace_range(span, value);
        return Ok(out);
    }
    if is_default {
        return Ok(text.to_owned());
    }

    const DESCRIPTION: &str = "<rdf:Description";
    let at = text.find(DESCRIPTION).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the sidecar has no rdf:Description to add the rating to; it was left unchanged",
        )
    })? + DESCRIPTION.len();
    let namespace = if text.contains("xmlns:xmp=") {
        ""
    } else {
        " xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\""
    };
    let mut out = text.to_owned();
    out.insert_str(at, &format!("{namespace} {tag}=\"{value}\""));
    Ok(out)
}

/// Write to a temporary file beside `path`, then rename it over `path`, so a
/// crash leaves either the old file or the new one, never half of one.
/// Also used for JPEG orientation saves (`orientation.rs`).
pub(crate) fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("patina-xmp-{name}-{}", std::process::id()));
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

    #[test]
    fn labels_written_to_disk_are_read_back() {
        let dir = TempDir::new("roundtrip");
        let image = dir.0.join("photo.jpg");
        let labels = ImageLabels {
            rating: 4,
            colour: ColourLabel::Green,
        };

        write_labels(&image, &labels).unwrap();

        assert!(dir.0.join("photo.jpg.xmp").is_file());
        assert_eq!(read_labels(&image), labels);
    }

    #[test]
    fn every_rating_and_label_round_trips() {
        let dir = TempDir::new("all");
        let image = dir.0.join("a.png");
        let colours = [
            ColourLabel::None,
            ColourLabel::Red,
            ColourLabel::Yellow,
            ColourLabel::Green,
            ColourLabel::Blue,
            ColourLabel::Purple,
        ];
        for rating in 0..=5 {
            for colour in colours {
                let labels = ImageLabels { rating, colour };
                write_labels(&image, &labels).unwrap();
                assert_eq!(read_labels(&image), labels, "rating {rating}, {colour:?}");
            }
        }
    }

    #[test]
    fn attribute_values_parse_with_either_quote() {
        let double = r#"<rdf:Description xmp:Rating="3" xmp:Label="Red"/>"#;
        let single = r#"<rdf:Description xmp:Rating='3' xmp:Label='Red'/>"#;
        let want = ImageLabels {
            rating: 3,
            colour: ColourLabel::Red,
        };
        assert_eq!(parse_xmp(double), want);
        assert_eq!(parse_xmp(single), want);
    }

    /// A sidecar as darktable writes it: double quotes, the same
    /// `photo.jpg.xmp` name Patina uses, and an edit history Patina must keep.
    const DARKTABLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:darktable="http://darktable.sf.net/"
    xmp:Rating="2"
    darktable:history_end="3">
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:operation="exposure" darktable:params="0000000000"/>
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn rating_a_foreign_sidecar_changes_only_the_rating() {
        let dir = TempDir::new("foreign");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.jpg.xmp");
        std::fs::write(&xmp, DARKTABLE).unwrap();

        write_labels(
            &image,
            &ImageLabels {
                rating: 5,
                colour: ColourLabel::None,
            },
        )
        .unwrap();

        let on_disk = std::fs::read_to_string(&xmp).unwrap();
        assert_eq!(
            on_disk,
            DARKTABLE.replace(r#"xmp:Rating="2""#, r#"xmp:Rating="5""#)
        );
    }

    #[test]
    fn a_label_is_added_to_a_foreign_sidecar_without_losing_anything() {
        let dir = TempDir::new("addlabel");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.jpg.xmp");
        std::fs::write(&xmp, DARKTABLE).unwrap();

        let labels = ImageLabels {
            rating: 2,
            colour: ColourLabel::Purple,
        };
        write_labels(&image, &labels).unwrap();

        let on_disk = std::fs::read_to_string(&xmp).unwrap();
        assert_eq!(
            on_disk,
            DARKTABLE.replace(
                "<rdf:Description rdf:about",
                r#"<rdf:Description xmp:Label="Purple" rdf:about"#
            )
        );
        assert_eq!(read_labels(&image), labels);
    }

    #[test]
    fn clearing_labels_keeps_a_foreign_sidecar() {
        let dir = TempDir::new("clearforeign");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.jpg.xmp");
        std::fs::write(&xmp, DARKTABLE).unwrap();

        write_labels(&image, &ImageLabels::default()).unwrap();

        let on_disk = std::fs::read_to_string(&xmp).unwrap();
        assert_eq!(
            on_disk,
            DARKTABLE.replace(r#"xmp:Rating="2""#, r#"xmp:Rating="0""#)
        );
    }

    #[test]
    fn clearing_labels_removes_patinas_own_sidecar() {
        let dir = TempDir::new("clearown");
        let image = dir.0.join("photo.jpg");
        write_labels(
            &image,
            &ImageLabels {
                rating: 3,
                colour: ColourLabel::Red,
            },
        )
        .unwrap();

        write_labels(&image, &ImageLabels::default()).unwrap();

        assert!(!dir.0.join("photo.jpg.xmp").exists());
    }

    #[test]
    fn the_xmp_namespace_is_declared_when_a_sidecar_lacks_it() {
        let dir = TempDir::new("nons");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.jpg.xmp");
        let bare = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"/></rdf:RDF></x:xmpmeta>"#;
        std::fs::write(&xmp, bare).unwrap();

        write_labels(
            &image,
            &ImageLabels {
                rating: 1,
                colour: ColourLabel::None,
            },
        )
        .unwrap();

        let on_disk = std::fs::read_to_string(&xmp).unwrap();
        assert!(
            on_disk.contains(r#"xmlns:xmp="http://ns.adobe.com/xap/1.0/""#),
            "{on_disk}"
        );
        assert!(
            on_disk.contains(r#"xmlns:dc="http://purl.org/dc/elements/1.1/""#),
            "{on_disk}"
        );
        assert_eq!(read_labels(&image).rating, 1);
    }

    #[test]
    fn a_sidecar_it_cannot_edit_is_left_unchanged() {
        let dir = TempDir::new("noedit");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.jpg.xmp");
        std::fs::write(&xmp, "not xmp at all").unwrap();

        let err = write_labels(
            &image,
            &ImageLabels {
                rating: 4,
                colour: ColourLabel::None,
            },
        );

        assert!(err.is_err());
        assert_eq!(std::fs::read_to_string(&xmp).unwrap(), "not xmp at all");
        assert!(!dir.0.join("photo.jpg.xmp.tmp").exists());
    }

    #[test]
    fn element_syntax_is_edited_in_place() {
        let dir = TempDir::new("element");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.jpg.xmp");
        let text =
            "<rdf:Description><xmp:Rating>1</xmp:Rating><dc:title>x</dc:title></rdf:Description>";
        std::fs::write(&xmp, text).unwrap();

        write_labels(
            &image,
            &ImageLabels {
                rating: 4,
                colour: ColourLabel::None,
            },
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&xmp).unwrap(),
            text.replace(">1<", ">4<")
        );
    }

    /// A sidecar as Lightroom writes it for `photo.CR2`: named `photo.xmp`,
    /// with develop settings Patina must keep.
    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="3"
    crs:Exposure2012="+0.35"
    crs:WhiteBalance="As Shot"/>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn a_lightroom_sidecar_is_read() {
        let dir = TempDir::new("lr-read");
        let image = dir.0.join("photo.CR2");
        std::fs::write(dir.0.join("photo.xmp"), LIGHTROOM).unwrap();

        assert_eq!(read_labels(&image).rating, 3);
    }

    #[test]
    fn a_lightroom_sidecar_is_written_in_place_and_no_second_one_appears() {
        let dir = TempDir::new("lr-write");
        let image = dir.0.join("photo.CR2");
        let xmp = dir.0.join("photo.xmp");
        std::fs::write(&xmp, LIGHTROOM).unwrap();

        let labels = ImageLabels {
            rating: 5,
            colour: ColourLabel::Red,
        };
        write_labels(&image, &labels).unwrap();

        assert_eq!(
            std::fs::read_to_string(&xmp).unwrap(),
            LIGHTROOM
                .replace(r#"xmp:Rating="3""#, r#"xmp:Rating="5""#)
                .replace(
                    "<rdf:Description rdf:about",
                    r#"<rdf:Description xmp:Label="Red" rdf:about"#
                )
        );
        assert!(!dir.0.join("photo.CR2.xmp").exists());
        assert_eq!(read_labels(&image), labels);
    }

    #[test]
    fn with_both_names_the_full_filename_one_is_used() {
        let dir = TempDir::new("both");
        let image = dir.0.join("photo.CR2");
        std::fs::write(dir.0.join("photo.xmp"), LIGHTROOM).unwrap();
        std::fs::write(dir.0.join("photo.CR2.xmp"), DARKTABLE).unwrap();

        assert_eq!(read_labels(&image).rating, 2);
        write_labels(
            &image,
            &ImageLabels {
                rating: 4,
                colour: ColourLabel::None,
            },
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.0.join("photo.CR2.xmp")).unwrap(),
            DARKTABLE.replace(r#"xmp:Rating="2""#, r#"xmp:Rating="4""#)
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("photo.xmp")).unwrap(),
            LIGHTROOM
        );
    }

    #[test]
    fn with_neither_name_the_full_filename_one_is_created() {
        let dir = TempDir::new("neither");
        let image = dir.0.join("photo.CR2");

        write_labels(
            &image,
            &ImageLabels {
                rating: 1,
                colour: ColourLabel::None,
            },
        )
        .unwrap();

        assert!(dir.0.join("photo.CR2.xmp").is_file());
        assert!(!dir.0.join("photo.xmp").exists());
    }

    /// A packet exactly as Patina writes it, but under `photo.xmp`, may belong
    /// to a RAW beside the JPEG: clearing keeps the file.
    #[test]
    fn clearing_labels_never_removes_a_stem_named_sidecar() {
        let dir = TempDir::new("clearstem");
        let image = dir.0.join("photo.jpg");
        let xmp = dir.0.join("photo.xmp");
        std::fs::write(&xmp, build_xmp_packet(3, "Red")).unwrap();

        write_labels(&image, &ImageLabels::default()).unwrap();

        assert_eq!(
            std::fs::read_to_string(&xmp).unwrap(),
            build_xmp_packet(0, "")
        );
    }

    fn touch(path: &Path) {
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn a_deleted_image_takes_its_own_sidecar_and_an_unshared_stem_one() {
        let dir = TempDir::new("sidecars-of");
        let raw = dir.0.join("photo.CR2");
        touch(&raw);
        touch(&dir.0.join("photo.CR2.xmp"));
        touch(&dir.0.join("photo.xmp"));
        touch(&dir.0.join("other.jpg"));

        assert_eq!(
            sidecars_of(&raw),
            vec![dir.0.join("photo.CR2.xmp"), dir.0.join("photo.xmp")]
        );
    }

    #[test]
    fn a_stem_sidecar_shared_with_another_file_stays() {
        let dir = TempDir::new("sidecars-shared");
        let jpg = dir.0.join("photo.jpg");
        touch(&jpg);
        touch(&dir.0.join("photo.CR2"));
        touch(&dir.0.join("photo.xmp"));

        assert_eq!(sidecars_of(&jpg), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_renamed_image_takes_its_sidecar_and_leaves_a_stem_one() {
        let dir = TempDir::new("rename-sidecar");
        let (old, new) = (dir.0.join("photo.jpg"), dir.0.join("photo.png"));
        std::fs::write(dir.0.join("photo.jpg.xmp"), DARKTABLE).unwrap();
        touch(&dir.0.join("photo.xmp"));

        rename_sidecar(&old, &new).unwrap();

        assert!(!dir.0.join("photo.jpg.xmp").exists());
        assert_eq!(
            std::fs::read_to_string(dir.0.join("photo.png.xmp")).unwrap(),
            DARKTABLE
        );
        assert!(dir.0.join("photo.xmp").exists());
    }

    #[test]
    fn a_renamed_image_never_replaces_a_sidecar_under_the_new_name() {
        let dir = TempDir::new("rename-sidecar-taken");
        let (old, new) = (dir.0.join("photo.jpg"), dir.0.join("photo.png"));
        std::fs::write(dir.0.join("photo.jpg.xmp"), DARKTABLE).unwrap();
        std::fs::write(dir.0.join("photo.png.xmp"), "already here").unwrap();

        assert!(rename_sidecar(&old, &new).is_err());

        assert_eq!(
            std::fs::read_to_string(dir.0.join("photo.jpg.xmp")).unwrap(),
            DARKTABLE
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("photo.png.xmp")).unwrap(),
            "already here"
        );
    }

    #[test]
    fn element_values_parse() {
        let text = "<xmp:Rating>2</xmp:Rating><xmp:Label>Blue</xmp:Label>";
        assert_eq!(
            parse_xmp(text),
            ImageLabels {
                rating: 2,
                colour: ColourLabel::Blue
            }
        );
    }
}
