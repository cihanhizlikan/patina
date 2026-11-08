//! nav.rs — Directory-based image navigation.
//!
//! Keeps the image files in the open file's directory, in Explorer's order
//! (explorer_order.rs). Navigation wraps around at both ends.

use crate::loader::is_supported;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct DirNavigator {
    /// All image files in the current directory, in Explorer's order.
    files: Vec<PathBuf>,
    /// Index of the currently displayed file.
    index: usize,
    /// The directory that `files` was built from. Stored explicitly so that
    /// the same-directory check in `set_current` does not have to infer it
    /// from `files.first()`, which would give a wrong answer after the
    /// single-entry fallback path resets the list.
    current_dir: Option<PathBuf>,
}

impl DirNavigator {
    /// Call whenever a new path is opened.
    /// Rebuilds the sibling list only when the directory actually changes.
    pub fn set_current(&mut self, path: PathBuf) {
        let new_dir = path.parent().map(Path::to_path_buf);

        if self.current_dir.as_deref() != new_dir.as_deref() {
            self.current_dir = new_dir;
            self.rebuild();
        }

        // Locate `path` in the sorted list. The fallback (replacing the list
        // with a single entry) should not occur in normal use — it would only
        // trigger if the file disappeared from the directory between the
        // rebuild and this position scan. Navigation still works; the sibling
        // list is restored the next time the directory changes.
        self.index = self
            .files
            .iter()
            .position(|p| *p == path)
            .unwrap_or_else(|| {
                self.files = vec![path];
                0
            });
    }

    /// Advance forward (wraps). Returns `None` if fewer than 2 images exist.
    pub fn next(&mut self) -> Option<PathBuf> {
        self.step(Direction::Forward)
    }

    /// Go back (wraps). Returns `None` if fewer than 2 images exist.
    pub fn prev(&mut self) -> Option<PathBuf> {
        self.step(Direction::Backward)
    }

    /// Peek at the next path without mutating state.
    pub fn peek_next(&self) -> Option<PathBuf> {
        self.peek(Direction::Forward)
    }

    /// Peek at the previous path without mutating state.
    pub fn peek_prev(&self) -> Option<PathBuf> {
        self.peek(Direction::Backward)
    }

    /// Remove a path (e.g. after deletion) and adjust the index so the next
    /// logical sibling becomes current.
    pub fn remove(&mut self, path: &Path) {
        if let Some(pos) = self.files.iter().position(|p| p.as_path() == path) {
            self.files.remove(pos);
            self.index = pos.min(self.files.len().saturating_sub(1));
        }
    }

    /// Update a path in the sibling list after a rename.
    /// Preserves the current index so navigation is unaffected.
    pub fn rename(&mut self, old_path: &Path, new_path: PathBuf) {
        if let Some(entry) = self.files.iter_mut().find(|p| p.as_path() == old_path) {
            *entry = new_path;
        }
    }

    /// The currently active path (may differ from the last `set_current` call
    /// if `remove` was called).
    pub fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    /// `path`'s place in the list, from 1, and the list's length. Usually
    /// `path` is the current file and this costs nothing; while a navigation
    /// is still loading it is not, and the list is searched.
    pub fn position_of(&self, path: &Path) -> Option<(usize, usize)> {
        let i = if self.current().is_some_and(|p| p == path) {
            self.index
        } else {
            self.files.iter().position(|p| p == path)?
        };
        Some((i + 1, self.files.len()))
    }

    /// Rebuild the sibling list in place after an OS-reported directory change.
    ///
    /// Re-locates the currently displayed file in the new sorted list so the
    /// index stays accurate regardless of how many files were added or removed.
    /// If the current file is no longer present the index is clamped — the
    /// caller is responsible for detecting this and navigating away.
    pub fn rebuild_current(&mut self, current_path: Option<&Path>) {
        self.rebuild();
        self.index = current_path
            .and_then(|p| self.files.iter().position(|f| f.as_path() == p))
            .unwrap_or_else(|| self.index.min(self.files.len().saturating_sub(1)));
    }

    // ── Private ───────────────────────────────────────────────────────────────

    /// Advance the index in `direction` and return the new current path.
    /// Returns `None` if fewer than 2 images exist (nothing to navigate to).
    fn step(&mut self, direction: Direction) -> Option<PathBuf> {
        if self.files.len() < 2 {
            return None;
        }
        self.index = self.wrapped_index(direction);
        Some(self.files[self.index].clone())
    }

    /// Peek at the neighbour in `direction` without mutating state.
    fn peek(&self, direction: Direction) -> Option<PathBuf> {
        (self.files.len() > 1).then(|| self.files[self.wrapped_index(direction)].clone())
    }

    /// Compute the neighbour index with wraparound.
    fn wrapped_index(&self, direction: Direction) -> usize {
        match direction {
            Direction::Forward => (self.index + 1) % self.files.len(),
            Direction::Backward => self.index.checked_sub(1).unwrap_or(self.files.len() - 1),
        }
    }

    /// Populate `files` from `current_dir`. Called only when the directory
    /// changes. `self.index` is not touched here — `set_current` sets it
    /// immediately after via `position()`.
    fn rebuild(&mut self) {
        let Some(dir) = self.current_dir.as_deref() else {
            self.files.clear();
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            self.files.clear();
            return;
        };

        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                // Use the DirEntry file type — already available from
                // the readdir() result on every major OS, no extra
                // stat() syscall needed. is_file() would cost one
                // syscall per file, i.e. 5000 disk hits on a cold
                // cache for a 5K-file folder.
                if !e.file_type().ok()?.is_file() {
                    return None;
                }
                let path = e.path();
                is_supported(&path).then_some(path)
            })
            .collect();

        crate::explorer_order::sort(&mut paths);
        self.files = paths;
    }
}

/// Direction for single-step navigation.
#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
}

// ── Test helpers (compiled only under `cargo test`) ───────────────────────────

#[cfg(test)]
impl DirNavigator {
    /// Build a navigator with a pre-loaded file list — no disk access.
    /// For the unit tests below.
    pub fn from_list(files: Vec<std::path::PathBuf>, current: &std::path::Path) -> Self {
        let index = files
            .iter()
            .position(|p| p.as_path() == current)
            .unwrap_or(0);
        let current_dir = current.parent().map(|p| p.to_path_buf());
        Self {
            files,
            index,
            current_dir,
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────
//
// Run all:               cargo test
// Run only nav tests:    cargo test nav
// Show output:           cargo test -- --nocapture

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn nav(files: &[&str], current: &str) -> DirNavigator {
        DirNavigator::from_list(
            files.iter().map(PathBuf::from).collect(),
            Path::new(current),
        )
    }

    // ── next() / prev() stepping ──────────────────────────────────────────────

    #[test]
    fn next_advances_one() {
        let mut n = nav(&["/d/a.jpg", "/d/b.jpg", "/d/c.jpg"], "/d/a.jpg");
        assert_eq!(n.next().unwrap(), PathBuf::from("/d/b.jpg"));
    }

    #[test]
    fn next_wraps_at_end() {
        let mut n = nav(&["/d/a.jpg", "/d/b.jpg", "/d/c.jpg"], "/d/c.jpg");
        assert_eq!(n.next().unwrap(), PathBuf::from("/d/a.jpg"));
    }

    #[test]
    fn prev_goes_back() {
        let mut n = nav(&["/d/a.jpg", "/d/b.jpg", "/d/c.jpg"], "/d/b.jpg");
        assert_eq!(n.prev().unwrap(), PathBuf::from("/d/a.jpg"));
    }

    #[test]
    fn prev_wraps_at_start() {
        let mut n = nav(&["/d/a.jpg", "/d/b.jpg", "/d/c.jpg"], "/d/a.jpg");
        assert_eq!(n.prev().unwrap(), PathBuf::from("/d/c.jpg"));
    }

    #[test]
    fn single_file_next_returns_none() {
        let mut n = nav(&["/d/only.jpg"], "/d/only.jpg");
        assert!(n.next().is_none());
    }

    #[test]
    fn single_file_prev_returns_none() {
        let mut n = nav(&["/d/only.jpg"], "/d/only.jpg");
        assert!(n.prev().is_none());
    }

    #[test]
    fn empty_list_returns_none() {
        let mut n = DirNavigator::default();
        assert!(n.next().is_none());
        assert!(n.prev().is_none());
    }

    // ── Full round-trip ───────────────────────────────────────────────────────

    #[test]
    fn next_full_cycle_returns_to_start() {
        let mut n = nav(
            &["/d/1.jpg", "/d/2.jpg", "/d/3.jpg", "/d/4.jpg"],
            "/d/1.jpg",
        );
        n.next();
        n.next();
        n.next();
        n.next();
        assert_eq!(n.current().unwrap(), &PathBuf::from("/d/1.jpg"));
    }

    #[test]
    fn prev_full_cycle_returns_to_start() {
        let mut n = nav(&["/d/1.jpg", "/d/2.jpg", "/d/3.jpg"], "/d/1.jpg");
        n.prev();
        n.prev();
        n.prev();
        assert_eq!(n.current().unwrap(), &PathBuf::from("/d/1.jpg"));
    }

    // ── rename() ─────────────────────────────────────────────────────────────

    #[test]
    fn rename_updates_current() {
        let mut n = nav(&["/d/old.jpg", "/d/other.jpg"], "/d/old.jpg");
        n.rename(Path::new("/d/old.jpg"), PathBuf::from("/d/new.jpg"));
        assert_eq!(n.current().unwrap(), &PathBuf::from("/d/new.jpg"));
    }

    #[test]
    fn rename_nonexistent_is_noop() {
        let mut n = nav(&["/d/a.jpg"], "/d/a.jpg");
        n.rename(Path::new("/d/ghost.jpg"), PathBuf::from("/d/new.jpg"));
        assert_eq!(n.current().unwrap(), &PathBuf::from("/d/a.jpg"));
    }

    // ── remove() ─────────────────────────────────────────────────────────────

    #[test]
    fn remove_shrinks_list() {
        let mut n = nav(&["/d/a.jpg", "/d/b.jpg", "/d/c.jpg"], "/d/a.jpg");
        n.remove(Path::new("/d/b.jpg"));
        assert_eq!(n.files.len(), 2);
    }

    #[test]
    fn removing_the_only_file_leaves_none_current() {
        let mut n = nav(&["/d/a.jpg"], "/d/a.jpg");
        n.remove(Path::new("/d/a.jpg"));
        assert_eq!(n.files.len(), 0);
        assert!(n.current().is_none());
    }

    #[test]
    fn remove_adjusts_index_to_valid_range() {
        let mut n = nav(&["/d/a.jpg", "/d/b.jpg", "/d/c.jpg"], "/d/c.jpg");
        n.remove(Path::new("/d/c.jpg"));
        // index must still point within the (now shorter) list, at the new last file
        assert_eq!(n.files.len(), 2);
        assert!(n.index < n.files.len());
        assert_eq!(n.current(), Some(&PathBuf::from("/d/b.jpg")));
    }
}
