//! xmp.rs — Read and write XMP sidecar files for star ratings and colour labels.
//!
//! XMP sidecars are plain XML files stored alongside the image with a `.xmp`
//! extension. They are the industry-standard way to store non-destructive
//! metadata that works across Lightroom, Capture One, and other tools.
//!
//! We write a minimal XMP packet containing only the fields we manage:
//!   xmp:Rating   (integer 0–5, 0 = unrated)
//!   xmp:Label    (string: "Red", "Yellow", "Green", "Blue", "Purple", or "")

use std::path::{Path, PathBuf};

// ── Colour label ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColourLabel {
    #[default] None,
    Red, Yellow, Green, Blue, Purple,
}

impl ColourLabel {
    pub fn xmp_name(self) -> &'static str {
        match self {
            Self::None   => "",      Self::Red    => "Red",
            Self::Yellow => "Yellow",Self::Green  => "Green",
            Self::Blue   => "Blue",  Self::Purple => "Purple",
        }
    }

    pub fn from_xmp(s: &str) -> Self {
        match s.trim() {
            "Red"    => Self::Red,    "Yellow" => Self::Yellow,
            "Green"  => Self::Green,  "Blue"   => Self::Blue,
            "Purple" => Self::Purple, _        => Self::None,
        }
    }

    /// egui colour for the label dot in the viewer.
    pub fn color(self) -> egui::Color32 {
        match self {
            Self::None   => egui::Color32::TRANSPARENT,
            Self::Red    => egui::Color32::from_rgb(0xE0, 0x55, 0x45),
            Self::Yellow => egui::Color32::from_rgb(0xE8, 0xC0, 0x40),
            Self::Green  => egui::Color32::from_rgb(0x50, 0xB8, 0x70),
            Self::Blue   => egui::Color32::from_rgb(0x50, 0x90, 0xD8),
            Self::Purple => egui::Color32::from_rgb(0xA0, 0x60, 0xD0),
        }
    }
}

// ── Image labels ──────────────────────────────────────────────────────────────

/// Star rating + colour label for one image. Persisted in an XMP sidecar.
#[derive(Clone, Debug, Default, PartialEq)]
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

/// Returns the path of the XMP sidecar for `image_path`.
/// e.g. `/photos/DSC001.jpg` → `/photos/DSC001.jpg.xmp`
/// We use the `.jpg.xmp` convention (full filename + .xmp) rather than
/// stripping the extension, to avoid collisions between `foo.jpg` and `foo.png`.
/// Returns the XMP sidecar path for an image.
///
/// Uses the full-filename convention: `photo.jpg` → `photo.jpg.xmp`.
/// This avoids collisions between `photo.jpg` and `photo.png` sharing `photo.xmp`.
pub fn sidecar_path(image_path: &Path) -> PathBuf {
    // Append ".xmp" to the full filename rather than replacing the extension.
    let filename = image_path.file_name().unwrap_or_default();
    let xmp_name = format!("{}.xmp", filename.to_string_lossy());
    image_path.with_file_name(xmp_name)
}

// ── Read ──────────────────────────────────────────────────────────────────────

/// Read labels from the XMP sidecar for `image_path`.
/// Returns `ImageLabels::default()` if no sidecar exists or parsing fails.
pub fn read_labels(image_path: &Path) -> ImageLabels {
    let xmp_path = sidecar_path(image_path);
    let Ok(text) = std::fs::read_to_string(&xmp_path) else { return ImageLabels::default(); };
    parse_xmp(&text)
}

fn parse_xmp(text: &str) -> ImageLabels {
    let mut labels = ImageLabels::default();

    // Text-search approach — avoids pulling in an XML parser dependency.
    // Handles both attribute style (xmp:Tag="value") and element style (<xmp:Tag>value</xmp:Tag>).
    if let Some(r) = find_xmp_value(text, "xmp:Rating") {
        labels.rating = r.trim().parse::<u8>().unwrap_or(0).min(5);
    }
    if let Some(l) = find_xmp_value(text, "xmp:Label") {
        labels.colour = ColourLabel::from_xmp(l.trim());
    }
    labels
}

/// Search for an XMP tag value using either attribute or element syntax.
fn find_xmp_value<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    extract_attr(text, tag).or_else(|| extract_element(text, tag))
}

/// XML allows either quote around an attribute value; `build_xmp_packet`
/// writes single quotes, Lightroom writes double.
fn extract_attr<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    ['"', '\''].into_iter().find_map(|quote| {
        let needle = format!("{tag}={quote}");
        let start  = text.find(&needle)? + needle.len();
        let end    = text[start..].find(quote)? + start;
        Some(&text[start..end])
    })
}

fn extract_element<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open  = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end   = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

// ── Write ─────────────────────────────────────────────────────────────────────

/// Write labels to the XMP sidecar for `image_path`.
/// Creates or overwrites the sidecar. Removes it if labels are empty.
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

pub fn write_labels(image_path: &Path, labels: &ImageLabels) -> std::io::Result<()> {
    let xmp_path = sidecar_path(image_path);

    if labels.is_empty() {
        // Remove sidecar if it exists and labels are cleared.
        if xmp_path.exists() { std::fs::remove_file(&xmp_path)?; }
        return Ok(());
    }

    let xmp_content = build_xmp_packet(labels.rating, labels.colour.xmp_name());
    std::fs::write(&xmp_path, xmp_content)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("patina-xmp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn labels_written_to_disk_are_read_back() {
        let dir   = TempDir::new("roundtrip");
        let image = dir.0.join("photo.jpg");
        let labels = ImageLabels { rating: 4, colour: ColourLabel::Green };

        write_labels(&image, &labels).unwrap();

        assert!(dir.0.join("photo.jpg.xmp").is_file());
        assert_eq!(read_labels(&image), labels);
    }

    #[test]
    fn every_rating_and_label_round_trips() {
        let dir   = TempDir::new("all");
        let image = dir.0.join("a.png");
        let colours = [ColourLabel::None, ColourLabel::Red, ColourLabel::Yellow,
                       ColourLabel::Green, ColourLabel::Blue, ColourLabel::Purple];
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
        let want = ImageLabels { rating: 3, colour: ColourLabel::Red };
        assert_eq!(parse_xmp(double), want);
        assert_eq!(parse_xmp(single), want);
    }

    #[test]
    fn element_values_parse() {
        let text = "<xmp:Rating>2</xmp:Rating><xmp:Label>Blue</xmp:Label>";
        assert_eq!(parse_xmp(text), ImageLabels { rating: 2, colour: ColourLabel::Blue });
    }
}
