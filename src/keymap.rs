//! keymap.rs — Every keyboard shortcut, in one table.
//!
//! `App::process_keyboard` dispatches from `KEYMAP` and the help overlay is
//! drawn from it, so the two cannot disagree. Before, they were written
//! separately and the help had drifted: it left out the 0–5 ratings and L.

use egui::{Key, Modifiers};

/// Where a binding works.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Viewer,
    Editor,
    Both,
}

impl Mode {
    pub fn includes(self, editor: bool) -> bool {
        match self {
            Mode::Viewer => !editor,
            Mode::Editor => editor,
            Mode::Both => true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Next,
    Prev,
    /// The digit pressed is the rating; 0 clears it.
    Rate,
    CycleLabel,
    EnterEditor,
    LeaveEditor,
    Slideshow,
    /// Stop the slideshow, else leave fullscreen.
    Back,
    Fullscreen,
    Help,
    Info,
    Open,
    Delete,
    Quit,
    Undo,
    Redo,
    Save,
    SaveAs,
    CycleGuides,
    /// Held, not pressed; `App::track_original_key` follows it by key position.
    ShowOriginal,
    ZoomToggle,
    ZoomFit,
    ZoomActual,
    ZoomIn,
    ZoomOut,
}

pub struct Binding {
    pub mode: Mode,
    pub keys: &'static [(Modifiers, Key)],
    pub action: Action,
    /// What the help overlay says it does.
    pub what: &'static str,
}

const NO: Modifiers = Modifiers::NONE;
const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::COMMAND
};

/// In the order the help lists them.
pub const KEYMAP: &[Binding] = &[
    // ── Viewer ────────────────────────────────────────────────────────────────
    Binding {
        mode: Mode::Viewer,
        keys: &[
            (NO, Key::ArrowRight),
            (NO, Key::ArrowDown),
            (NO, Key::PageDown),
        ],
        action: Action::Next,
        what: "Next image",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(NO, Key::ArrowLeft), (NO, Key::ArrowUp), (NO, Key::PageUp)],
        action: Action::Prev,
        what: "Previous image",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[
            (NO, Key::Num1),
            (NO, Key::Num2),
            (NO, Key::Num3),
            (NO, Key::Num4),
            (NO, Key::Num5),
        ],
        action: Action::Rate,
        what: "Rate one to five stars",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(NO, Key::Num0)],
        action: Action::Rate,
        what: "Clear the rating",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(NO, Key::L)],
        action: Action::CycleLabel,
        what: "Next colour label",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(NO, Key::E)],
        action: Action::EnterEditor,
        what: "Open the editor",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(CTRL, Key::O)],
        action: Action::Open,
        what: "Open an image",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(NO, Key::S)],
        action: Action::Slideshow,
        what: "Start or stop the slideshow",
    },
    Binding {
        mode: Mode::Viewer,
        keys: &[(NO, Key::Escape)],
        action: Action::Back,
        what: "Stop the slideshow, or leave fullscreen",
    },
    // ── Editor ────────────────────────────────────────────────────────────────
    Binding {
        mode: Mode::Editor,
        keys: &[(NO, Key::E), (NO, Key::Escape)],
        action: Action::LeaveEditor,
        what: "Back to the viewer",
    },
    Binding {
        mode: Mode::Editor,
        keys: &[(CTRL, Key::Z)],
        action: Action::Undo,
        what: "Undo",
    },
    Binding {
        mode: Mode::Editor,
        keys: &[(CTRL_SHIFT, Key::Z), (CTRL, Key::Y)],
        action: Action::Redo,
        what: "Redo",
    },
    Binding {
        mode: Mode::Editor,
        keys: &[(CTRL, Key::S)],
        action: Action::Save,
        what: "Save",
    },
    Binding {
        mode: Mode::Editor,
        keys: &[(CTRL_SHIFT, Key::S)],
        action: Action::SaveAs,
        what: "Save As",
    },
    Binding {
        mode: Mode::Editor,
        keys: &[(NO, Key::G)],
        action: Action::CycleGuides,
        what: "Guides: off, thirds, golden ratio",
    },
    Binding {
        mode: Mode::Editor,
        keys: &[(NO, Key::Backslash)],
        action: Action::ShowOriginal,
        what: "Hold to see the original",
    },
    // ── Both ──────────────────────────────────────────────────────────────────
    Binding {
        mode: Mode::Both,
        keys: &[(NO, Key::Z)],
        action: Action::ZoomToggle,
        what: "Fit, or 100% where the pointer is",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(CTRL, Key::Plus), (CTRL, Key::Equals)],
        action: Action::ZoomIn,
        what: "Zoom in",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(CTRL, Key::Minus)],
        action: Action::ZoomOut,
        what: "Zoom out",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(CTRL, Key::Num1)],
        action: Action::ZoomActual,
        what: "100%: one image pixel per screen pixel",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(CTRL, Key::Num0)],
        action: Action::ZoomFit,
        what: "Fit to the window",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(NO, Key::F), (NO, Key::F11)],
        action: Action::Fullscreen,
        what: "Fullscreen (stops a slideshow)",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(NO, Key::I)],
        action: Action::Info,
        what: "Image info (EXIF)",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(NO, Key::Delete)],
        action: Action::Delete,
        what: "Move to the Recycle Bin",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(NO, Key::H)],
        action: Action::Help,
        what: "This help",
    },
    Binding {
        mode: Mode::Both,
        keys: &[(NO, Key::Q)],
        action: Action::Quit,
        what: "Quit",
    },
];

/// Mouse gestures, listed in the help after the keys. They have no handler
/// here: the canvas and the sliders handle them.
pub const MOUSE_HELP: &[(Mode, &str, &str)] = &[
    (Mode::Both, "Ctrl+Scroll", "Zoom in or out at the pointer"),
    (Mode::Both, "Drag", "Move a zoomed image"),
    (Mode::Editor, "Double-click", "Reset a slider"),
];

/// The rating a digit key means.
pub fn digit(key: Key) -> u8 {
    match key {
        Key::Num1 => 1,
        Key::Num2 => 2,
        Key::Num3 => 3,
        Key::Num4 => 4,
        Key::Num5 => 5,
        _ => 0,
    }
}

/// How the help shows a binding's keys: `Ctrl+Shift+Z  Ctrl+Y`.
pub fn keys_label(keys: &[(Modifiers, Key)]) -> String {
    keys.iter()
        .map(|&(m, k)| {
            let mut s = String::new();
            if m.command || m.ctrl {
                s.push_str("Ctrl+");
            }
            if m.shift {
                s.push_str("Shift+");
            }
            s.push_str(key_name(k));
            s
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn key_name(k: Key) -> &'static str {
    match k {
        Key::ArrowRight => "→",
        Key::ArrowLeft => "←",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::PageUp => "PgUp",
        Key::PageDown => "PgDn",
        Key::Delete => "Del",
        Key::Escape => "Esc",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Plus => "+",
        Key::Equals => "=",
        Key::Minus => "\u{2212}",
        _ => k.name(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two bindings on one key in one mode: only the first would ever run.
    #[test]
    fn no_key_does_two_things_in_one_mode() {
        for editor in [false, true] {
            let mut seen: Vec<(Modifiers, Key)> = Vec::new();
            for b in KEYMAP.iter().filter(|b| b.mode.includes(editor)) {
                for k in b.keys {
                    assert!(
                        !seen.contains(k),
                        "{} bound twice (editor: {editor})",
                        keys_label(&[*k])
                    );
                    seen.push(*k);
                }
            }
        }
    }

    /// Tab moves keyboard focus between controls, as everywhere else on the
    /// desktop; it used to cycle the guide type.
    #[test]
    fn tab_is_left_to_focus() {
        assert!(KEYMAP
            .iter()
            .flat_map(|b| b.keys)
            .all(|&(_, k)| k != Key::Tab));
    }

    #[test]
    fn labels_read_like_the_keyboard() {
        assert_eq!(
            keys_label(&[(CTRL_SHIFT, Key::Z), (CTRL, Key::Y)]),
            "Ctrl+Shift+Z  Ctrl+Y"
        );
        assert_eq!(
            keys_label(&[(NO, Key::ArrowRight), (NO, Key::PageDown)]),
            "→  PgDn"
        );
    }
}
