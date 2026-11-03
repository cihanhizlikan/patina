//! explorer_order.rs — file names in the order Explorer shows them.
//!
//! On Windows this is Explorer's own comparison, `StrCmpLogicalW`: numbers by
//! value, the user's locale for letters (in Turkish `ç` follows `cz`), and
//! Windows' weights for punctuation (`IMG_0002.jpg` before `IMG_0002_edit.jpg`).
//! No fixed rule reproduces it — the best safe one measured, ICU4X with the
//! locale's tailoring, put 7 of 71 realistic names elsewhere — so this is the
//! one module allowed `unsafe` (docs/settled-decisions.md). Elsewhere, a
//! natural sort on lowercased names.
#![allow(unsafe_code)]

use std::cmp::Ordering;
use std::path::PathBuf;

/// Sort `paths` by file name in Explorer's order. Names Explorer counts as
/// equal fall back to their raw comparison, so the order never depends on
/// the order the directory listed them in.
pub fn sort(paths: &mut [PathBuf]) {
    let mut keyed: Vec<(Key, PathBuf)> = paths.iter().map(|p| (key(p), p.clone())).collect();
    keyed.sort_by(|(a, pa), (b, pb)| compare(a, b).then_with(|| pa.cmp(pb)));
    for (slot, (_, path)) in paths.iter_mut().zip(keyed) {
        *slot = path;
    }
}

#[cfg(windows)]
type Key = Vec<u16>;

/// The file name as NUL-terminated UTF-16, encoded once per file rather than
/// once per comparison.
#[cfg(windows)]
fn key(path: &std::path::Path) -> Key {
    use std::os::windows::ffi::OsStrExt;
    path.file_name()
        .unwrap_or_default()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn compare(a: &Key, b: &Key) -> Ordering {
    // SAFETY: `a` and `b` are UTF-16 buffers built by `key`, each ending in
    // the NUL it appended (a Windows file name cannot hold one earlier), and
    // both live until after the call. StrCmpLogicalW only reads up to that
    // NUL, keeps no pointer, and returns <0, 0 or >0.
    let r = unsafe { windows_sys::Win32::UI::Shell::StrCmpLogicalW(a.as_ptr(), b.as_ptr()) };
    r.cmp(&0)
}

#[cfg(not(windows))]
type Key = String;

#[cfg(not(windows))]
fn key(path: &std::path::Path) -> Key {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}

/// Runs of ASCII digits compare by value, so `img2` < `img10`.
#[cfg(not(windows))]
fn compare(a: &Key, b: &Key) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(ac), Some(bc)) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                let ord = number(&mut ai).cmp(&number(&mut bi));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ac), Some(bc)) => {
                let ord = ac.cmp(&bc);
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Consume a run of ASCII digits as a `u64`, saturating on overflow.
#[cfg(not(windows))]
fn number(iter: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u64 {
    let mut n = 0u64;
    while let Some(&c) = iter.peek() {
        let Some(digit) = c.to_digit(10) else { break };
        n = n.saturating_mul(10).saturating_add(digit as u64);
        iter.next();
    }
    n
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut paths: Vec<PathBuf> = names
            .iter()
            .map(|n| PathBuf::from(r"C:\x").join(n))
            .collect();
        sort(&mut paths);
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// Explorer's order for names a photo folder holds, recorded from
    /// `StrCmpLogicalW` through PowerShell, outside Patina. Locale-free cases
    /// only, so the test passes under any Windows language.
    #[test]
    fn photo_names_sort_as_explorer_shows_them() {
        let want = [
            "001.jpg",
            "01.jpg",
            "1.jpg",
            "2.jpg",
            "10.jpg",
            "2024-01-05 9.30.jpg",
            "2024-01-05 12.30.jpg",
            "IMG_0001.jpg",
            "IMG_0002 - Copy.jpg",
            "IMG_0002 (2).jpg",
            "IMG_0002.edit.jpg",
            "IMG_0002.jpg",
            "IMG_0002_edit.jpg",
            "IMG_0002-edit.jpg",
            "img_2.jpg",
            "IMG_0010.jpg",
            "photo (1).jpg",
            "photo (2).jpg",
            "photo (10).jpg",
            "photo.jpg",
            "photo1.jpg",
            "photo9.jpg",
            "photo10.jpg",
            "Zebra.jpg",
        ];
        let mut shuffled = want.to_vec();
        shuffled.reverse();
        shuffled.swap(3, 17);
        assert_eq!(sorted(&shuffled), want);
    }

    /// An accented letter sorts beside its base letter, not after `z` as the
    /// code-point sort did. True in every Windows locale.
    #[test]
    fn accented_names_sort_beside_their_letter() {
        assert_eq!(
            sorted(&["zebra.jpg", "ecz.jpg", "éclair.jpg", "eclair.jpg"]),
            ["eclair.jpg", "éclair.jpg", "ecz.jpg", "zebra.jpg"]
        );
    }

    /// Names Explorer counts as equal still sort the same way every time.
    #[test]
    fn the_order_does_not_depend_on_the_listing() {
        let a = sorted(&["a.jpg", "A.jpg", "b.jpg"]);
        let b = sorted(&["b.jpg", "A.jpg", "a.jpg"]);
        assert_eq!(a, b);
    }
}
