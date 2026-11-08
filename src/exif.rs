//! exif.rs — EXIF/IPTC metadata extraction for Patina.
//!
//! Reads EXIF tags from any file that embeds them (JPEG, TIFF, RAW formats).
//! Returns a flat list of display-ready key/value pairs in a priority-ordered
//! sequence reflecting how photographers actually use EXIF data.

use std::path::Path;

/// A single metadata field for display in the EXIF panel.
#[derive(Clone, Debug)]
pub struct ExifField {
    pub label: &'static str,
    pub value: String,
}

/// All metadata extracted from one image file.
/// Fields are ordered by display priority (most useful first).
#[derive(Clone, Debug, Default)]
pub struct ImageMetadata {
    pub fields: Vec<ExifField>,
    /// GPS coordinates if present, for future map integration.
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

impl ImageMetadata {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Read EXIF/IPTC metadata from `path`.
/// Returns an empty `ImageMetadata` if the file has no embedded metadata
/// or the format is not supported — never returns an error to the caller.
pub fn read_metadata(path: &Path) -> ImageMetadata {
    read_metadata_inner(path).unwrap_or_default()
}

fn read_metadata_inner(path: &Path) -> Option<ImageMetadata> {
    use exif::{In, Reader, Tag};

    let file = std::fs::File::open(path).ok()?;
    let mut br = std::io::BufReader::new(file);
    let exif = Reader::new().read_from_container(&mut br).ok()?;

    let mut meta = ImageMetadata::default();

    // Helper: get a tag's display value or None
    let get = |tag: Tag, ifd: In| -> Option<String> {
        exif.get_field(tag, ifd)
            .map(|f| f.display_value().to_string())
    };
    let get0 = |tag: Tag| get(tag, In::PRIMARY);

    // ── Priority order: what a photographer looks at first ────────────────────

    // 1. Camera model — always the first thing you want to know
    if let Some(v) = get0(Tag::Model) {
        meta.fields.push(ExifField {
            label: "Camera",
            value: clean_string(&v),
        });
    }
    if let Some(v) = get0(Tag::Make) {
        // Append make only if not already present in Model
        let make = clean_string(&v);
        if let Some(cam) = meta.fields.iter_mut().find(|f| f.label == "Camera") {
            if !cam.value.to_lowercase().contains(&make.to_lowercase()) {
                cam.value = format!("{make} {}", cam.value);
            }
        }
    }

    // 2. Lens
    if let Some(v) = get0(Tag::LensModel) {
        meta.fields.push(ExifField {
            label: "Lens",
            value: clean_string(&v),
        });
    }

    // 3. Capture settings (the exposure triangle)
    if let Some(v) = get0(Tag::ExposureTime) {
        meta.fields.push(ExifField {
            label: "Shutter",
            value: format_shutter(&v),
        });
    }
    if let Some(v) = get0(Tag::FNumber) {
        meta.fields.push(ExifField {
            label: "Aperture",
            value: format_aperture(&v),
        });
    }
    if let Some(v) = get0(Tag::PhotographicSensitivity) {
        meta.fields.push(ExifField {
            label: "ISO",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::FocalLength) {
        meta.fields.push(ExifField {
            label: "Focal length",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::FocalLengthIn35mmFilm) {
        meta.fields.push(ExifField {
            label: "35mm equiv.",
            value: format!("{v}mm"),
        });
    }

    // 4. Date/time
    if let Some(v) = get0(Tag::DateTimeOriginal).or_else(|| get0(Tag::DateTime)) {
        meta.fields.push(ExifField {
            label: "Captured",
            value: format_datetime(&v),
        });
    }

    // 5. Image properties
    if let Some(v) = get0(Tag::PixelXDimension) {
        let h = get0(Tag::PixelYDimension).unwrap_or_default();
        meta.fields.push(ExifField {
            label: "Resolution",
            value: format!("{v} × {h}"),
        });
    }
    if let Some(v) = get0(Tag::ColorSpace) {
        meta.fields.push(ExifField {
            label: "Color space",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::WhiteBalance) {
        meta.fields.push(ExifField {
            label: "White balance",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::Flash) {
        meta.fields.push(ExifField {
            label: "Flash",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::MeteringMode) {
        meta.fields.push(ExifField {
            label: "Metering",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::ExposureProgram) {
        meta.fields.push(ExifField {
            label: "Exposure mode",
            value: v,
        });
    }
    if let Some(v) = get0(Tag::ExposureBiasValue) {
        meta.fields.push(ExifField {
            label: "Exposure comp.",
            value: format_ev(&v),
        });
    }

    // 6. GPS
    let gps_lat = read_gps_coord(&exif, Tag::GPSLatitude, Tag::GPSLatitudeRef);
    let gps_lon = read_gps_coord(&exif, Tag::GPSLongitude, Tag::GPSLongitudeRef);
    if let (Some(lat), Some(lon)) = (gps_lat, gps_lon) {
        meta.fields.push(ExifField {
            label: "GPS",
            value: format!("{:.6}°, {:.6}°", lat, lon),
        });
        meta.gps_lat = Some(lat);
        meta.gps_lon = Some(lon);
    }

    // 7. Copyright / credit
    if let Some(v) = get0(Tag::Copyright) {
        if !v.trim().is_empty() {
            meta.fields.push(ExifField {
                label: "Copyright",
                value: clean_string(&v),
            });
        }
    }
    if let Some(v) = get0(Tag::Artist) {
        if !v.trim().is_empty() {
            meta.fields.push(ExifField {
                label: "Artist",
                value: clean_string(&v),
            });
        }
    }

    // 8. Software (last — rarely needed)
    if let Some(v) = get0(Tag::Software) {
        meta.fields.push(ExifField {
            label: "Software",
            value: clean_string(&v),
        });
    }

    Some(meta)
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/// Strip null bytes, leading/trailing whitespace, and surrounding quotes.
fn clean_string(s: &str) -> String {
    s.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace() || c == '\0')
        .to_string()
}

/// Format shutter speed as a fraction when < 1s, otherwise as decimal seconds.
fn format_shutter(s: &str) -> String {
    // exif display_value returns "1/250 s" or "2 s" style already — clean it up.
    s.replace(" s", "s")
}

/// Format aperture as f/2.8 style.
fn format_aperture(s: &str) -> String {
    // display_value returns "2.8" — prefix with f/
    if s.contains('/') {
        s.to_string()
    } else {
        format!("f/{s}")
    }
}

/// Simplify datetime "2024:06:15 14:23:01" → "15 Jun 2024  14:23"
fn format_datetime(s: &str) -> String {
    // EXIF datetime format: "YYYY:MM:DD HH:MM:SS"
    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    if parts.len() == 2 {
        let date_parts: Vec<&str> = parts[0].split(':').collect();
        if date_parts.len() == 3 {
            let months = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let month_idx = date_parts[1]
                .parse::<usize>()
                .unwrap_or(1)
                .saturating_sub(1);
            let month = months.get(month_idx).copied().unwrap_or("?");
            let time = parts[1]
                .char_indices()
                .nth(5)
                .map_or(parts[1], |(i, _)| &parts[1][..i]); // HH:MM
            return format!("{} {} {}  {}", date_parts[2], month, date_parts[0], time);
        }
    }
    s.to_string()
}

/// Format EV compensation with + prefix for positive values.
fn format_ev(s: &str) -> String {
    if let Ok(v) = s.parse::<f64>() {
        if v > 0.0 {
            return format!("+{:.1} EV", v);
        }
        if v < 0.0 {
            return format!("{:.1} EV", v);
        }
        return "0 EV".to_string();
    }
    s.to_string()
}

/// Read a GPS coordinate (lat or lon) as signed decimal degrees.
fn read_gps_coord(exif: &exif::Exif, tag: exif::Tag, ref_tag: exif::Tag) -> Option<f64> {
    use exif::{In, Value};
    let field = exif.get_field(tag, In::PRIMARY)?;
    let ref_val = exif
        .get_field(ref_tag, In::PRIMARY)
        .map(|f| f.display_value().to_string())
        .unwrap_or_default();

    let values = match &field.value {
        Value::Rational(v) => v.clone(),
        _ => return None,
    };
    if values.len() < 3 {
        return None;
    }

    let deg = values[0].to_f64();
    let min = values[1].to_f64() / 60.0;
    let sec = values[2].to_f64() / 3600.0;
    let mut dd = deg + min + sec;

    if ref_val.contains('S') || ref_val.contains('W') {
        dd = -dd;
    }
    Some(dd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_time_reads_day_month_year_and_minutes() {
        assert_eq!(format_datetime("2024:06:15 14:23:01"), "15 Jun 2024  14:23");
    }

    /// The time is cut at five characters, not five bytes: a malformed
    /// file's non-ASCII byte there must not split a character and panic.
    #[test]
    fn a_non_ascii_time_is_cut_at_a_character() {
        assert_eq!(format_datetime("2024:06:15 12:3é:01"), "15 Jun 2024  12:3é");
    }
}
