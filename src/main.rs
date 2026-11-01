//! main.rs — eframe/egui rendering, cross-platform.
//!
//! Architecture
//! ============
//! - eframe manages the window, event loop, and wgpu/egui backend.
//! - Image pixels → wgpu texture → registered with egui via `register_native_texture`
//!   → displayed as `egui::Image` in the CentralPanel. Zero pixel copies.
//! - Editor panel → egui SidePanel (see ui.rs).
//! - All app logic (loading, navigation, editing) lives in App and its methods.
//!
//! Cross-platform notes
//! ====================
//! - No #[cfg(windows)] blocks. Win32 hooks from the old winit build are removed;
//!   eframe handles window management natively on each platform.
//! - rfd (file dialogs) is cross-platform.
//! - build.rs / winres embeds the Windows icon but is a no-op on Linux/macOS.

// No console window in release builds: on Windows every launch opened one
// beside the app. The log goes to a file instead (logging.rs). Debug builds
// and `cargo run` keep the console. Other platforms ignore the attribute.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Patina's log line: `log::info!`, timestamped and written by logging.rs.
macro_rules! log {
    ($($arg:tt)*) => { ::log::info!($($arg)*) };
}

mod editor;
mod exif;
mod hud;
mod keymap;
mod loader;
mod logging;
mod nav;
mod orientation;
mod save;
mod ui;
mod xmp;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossbeam_channel::{bounded, Receiver, Sender};
use editor::{apply_edits, EditParams, EditorState};
use eframe::egui_wgpu::wgpu;
use exif::ImageMetadata;
use loader::{load_image, DecodedImage, LoadOutcome};
use nav::DirNavigator;
use notify_debouncer_mini::{new_debouncer, Debouncer};
use ui::{draw_panel, Icons, PanelAction, PanelInput};
use xmp::{ColourLabel, ImageLabels};

// ── Type aliases ──────────────────────────────────────────────────────────────

type DirWatcher = Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>;

// ── Constants ─────────────────────────────────────────────────────────────────

const APP_NAME: &str = "Patina";
const DIR_WATCH_DEBOUNCE: Duration = Duration::from_millis(50);
/// Slider debounce: wait this long after the last change before re-encoding.
const SLIDER_DEBOUNCE_MS: u128 = 80;
/// Minimum window dimensions on startup.
const MIN_WINDOW: (f32, f32) = (400.0, 300.0);
/// Default window size when no image is provided at launch.
const DEFAULT_WINDOW: (u32, u32) = (900, 680);
/// How long each image is shown during slideshow.
const SLIDESHOW_INTERVAL: Duration = Duration::from_secs(5);

// ── Inter-thread messages ─────────────────────────────────────────────────────

#[derive(Debug)]
enum Msg {
    Loaded {
        path: PathBuf,
        image: DecodedImage,
        mode: OpenMode,
    },
    WrongExtension {
        path: PathBuf,
        image: DecodedImage,
        mode: OpenMode,
        correct_ext: String,
    },
    Unsupported {
        path: PathBuf,
        reason: String,
        mode: OpenMode,
    },
    Preloaded {
        path: PathBuf,
        image: DecodedImage,
    },
    Deleted {
        path: PathBuf,
        result: Result<(), String>,
    },
    MetadataLoaded {
        path: PathBuf,
        meta: ImageMetadata,
    },
    DirectoryChanged,
}

#[derive(Default)]
struct DrainResult {
    resize_to: Option<(u32, u32)>,
    error_msg: Option<String>,
    rename_offer: Option<(PathBuf, String)>,
}

// ── Modal dialog variants ─────────────────────────────────────────────────────

#[derive(Debug)]
enum ModalDialog {
    SaveOnExit {
        save_and_quit: bool,
    },
    /// Holds the file it asks about, so Delete trashes that file even if the
    /// view has moved on since the dialog opened.
    ConfirmDelete {
        path: PathBuf,
    },
    OfferRename {
        old_path: PathBuf,
        new_path: PathBuf,
        old_ext: String,
        new_ext: String,
    },
    Error {
        message: String,
    },
}

// ── GPU image texture ─────────────────────────────────────────────────────────

/// Wgpu texture registered with egui via `register_native_texture`.
/// The `texture_id` is a stable handle — egui never touches the pixel data.
struct ImageTexture {
    texture: wgpu::Texture,
    texture_id: egui::TextureId,
    width: u32,
    height: u32,
}

// ── Save As modal state ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OutputFormat {
    Png,
    Jpeg,
    WebP,
    Tiff,
    Bmp,
}

impl OutputFormat {
    const ALL: &'static [Self] = &[Self::Png, Self::Jpeg, Self::WebP, Self::Tiff, Self::Bmp];

    fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Tiff => "TIFF",
            Self::Bmp => "BMP",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
            Self::Tiff => "tif",
            Self::Bmp => "bmp",
        }
    }

    fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("jpg") | Some("jpeg") => Self::Jpeg,
            Some("webp") => Self::WebP,
            Some("tif") | Some("tiff") => Self::Tiff,
            Some("bmp") => Self::Bmp,
            _ => Self::Png,
        }
    }
}

/// State for the Save As modal dialog.
/// Constructed via `App::default_save_as_modal()`; confirmed via `App::commit_save_as()`.
struct SaveAsModal {
    /// Filename without extension.
    stem: String,
    format: OutputFormat,
    directory: PathBuf,
    /// JPEG/WebP encode quality in [1, 100].
    quality: u8,
    /// When set, the exported image is resized to fit within this square.
    max_dim: Option<u32>,
    preset: ExportPreset,
    /// An existing file the dialog has warned about. Saving to exactly this
    /// path replaces it; any change to the folder, name or format makes a
    /// new target and asks again.
    replace: Option<PathBuf>,
}

/// Built-in export presets with format, quality, and optional max dimension.
/// Selecting a preset fills the SaveAsModal fields; editing any field switches to Custom.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum ExportPreset {
    /// User-configured values — no automatic overrides.
    #[default]
    Custom,
    /// JPEG at quality 85, resized to fit within 2048×2048px.
    Web,
    /// PNG lossless, no resize — maximum fidelity for print reproduction.
    Print,
    /// WebP at quality 90, no resize — compact archival format.
    Archive,
}

impl ExportPreset {
    const ALL: &'static [(Self, &'static str)] = &[
        (Self::Custom, "Custom"),
        (Self::Web, "Web (JPEG 85%, ≤2048px)"),
        (Self::Print, "Print (PNG lossless)"),
        (Self::Archive, "Archive (WebP 90%)"),
    ];

    fn apply(self, modal: &mut SaveAsModal) {
        match self {
            Self::Custom => {}
            Self::Web => {
                modal.format = OutputFormat::Jpeg;
                modal.quality = 85;
                modal.max_dim = Some(2048);
            }
            Self::Print => {
                modal.format = OutputFormat::Png;
                modal.quality = 100;
                modal.max_dim = None;
            }
            Self::Archive => {
                modal.format = OutputFormat::WebP;
                modal.quality = 90;
                modal.max_dim = None;
            }
        }
    }
}

impl SaveAsModal {
    fn full_filename(&self) -> String {
        format!("{}.{}", self.stem.trim(), self.format.extension())
    }

    fn full_path(&self) -> PathBuf {
        self.directory.join(self.full_filename())
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OpenMode {
    FirstImage,
    Navigate,
}

/// The guide overlay on the editor canvas; G steps through them in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Guides {
    #[default]
    Off,
    Thirds,
    Golden,
}

impl Guides {
    fn next(self) -> Self {
        match self {
            Guides::Off => Guides::Thirds,
            Guides::Thirds => Guides::Golden,
            Guides::Golden => Guides::Off,
        }
    }
}

struct App {
    // ── Image display ─────────────────────────────────────────────────────────
    current_path: Option<PathBuf>,
    current_image: Option<DecodedImage>,
    frame_index: usize,
    next_frame_at: Option<Instant>,
    image_texture: Option<ImageTexture>,
    last_uploaded_gen: u64,
    last_uploaded_frame: usize,

    // ── Navigation & loading ──────────────────────────────────────────────────
    navigator: DirNavigator,
    msg_tx: Sender<Msg>,
    msg_rx: Receiver<Msg>,
    loading_path: Option<PathBuf>,
    /// When the load on screen started, for the loading bar.
    loading_since: Option<Instant>,
    /// The file on screen would not open: its reason, shown as a card.
    bad_file: Option<String>,
    /// Last navigation or mouse move over the canvas: the HUD shows from it.
    hud_since: Option<Instant>,
    toast: hud::Toast,
    preload_cache: HashMap<PathBuf, DecodedImage>,
    deleting: bool,
    dir_watcher: Option<DirWatcher>,
    watched_dir: Option<PathBuf>,

    // ── Window state ──────────────────────────────────────────────────────────
    fullscreen: bool,
    pending_fullscreen_toggle: bool,
    maximized: bool,
    pre_maximize_size: Option<egui::Vec2>,
    image_gen: u64,
    /// Resizes the window exactly once on the first image launch.
    has_shown_first_image: bool,

    // ── Slideshow ─────────────────────────────────────────────────────────────
    slideshow_active: bool,
    slideshow_next_at: Option<Instant>,

    // ── Canvas view state (editor mode only) ──────────────────────────────────
    /// Zoom multiplier: 1.0 = fit to canvas. Ctrl+scroll adjusts; Enter resets.
    zoom: f32,
    /// Pan offset in logical pixels from the centred position.
    pan: egui::Vec2,
    guides: Guides,
    /// The window title last sent, so it is sent only when it changes.
    title: String,

    // ── Metadata ──────────────────────────────────────────────────────────────
    /// Cached EXIF/IPTC metadata for the current image.
    metadata: Option<ImageMetadata>,
    /// Star rating and colour label for the current image (XMP sidecar).
    labels: ImageLabels,

    // ── Editor ────────────────────────────────────────────────────────────────
    editor_state: Option<EditorState>,
    initial_editor_params: Option<EditParams>,
    /// The preset chip being previewed on the canvas, if any.
    preview: Option<editor::Preset>,
    icon_cache: Option<Icons>,

    // ── Async pixel editing ───────────────────────────────────────────────────
    edit_rx: Option<crossbeam_channel::Receiver<(u64, DecodedImage)>>,
    edit_gen: u64,
    last_slider_change: Option<Instant>,

    // ── Deferred / modal actions ──────────────────────────────────────────────
    modal: Option<ModalDialog>,
    save_as_modal: Option<SaveAsModal>,
    pending_error: Option<String>,
    pending_rename: Option<(PathBuf, String)>,
    pending_delete: Option<PathBuf>,
    pending_save_and_exit: Option<bool>,
    /// Discard was chosen in a "Save changes?" that a quit opened.
    pending_close: bool,
    show_help: bool,
    /// The EXIF card over the canvas (I).
    show_info: bool,
}

impl App {
    fn new(msg_tx: Sender<Msg>, msg_rx: Receiver<Msg>) -> Self {
        App {
            current_path: None,
            current_image: None,
            frame_index: 0,
            next_frame_at: None,
            image_texture: None,
            last_uploaded_gen: u64::MAX,
            last_uploaded_frame: usize::MAX,
            navigator: DirNavigator::default(),
            msg_tx,
            msg_rx,
            loading_path: None,
            loading_since: None,
            bad_file: None,
            hud_since: None,
            toast: hud::Toast::default(),
            preload_cache: HashMap::new(),
            deleting: false,
            dir_watcher: None,
            watched_dir: None,
            fullscreen: false,
            pending_fullscreen_toggle: false,
            maximized: false,
            pre_maximize_size: None,
            image_gen: 0,
            has_shown_first_image: false,
            slideshow_active: false,
            slideshow_next_at: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            guides: Guides::Off,
            title: String::new(),
            metadata: None,
            labels: ImageLabels::default(),
            editor_state: None,
            initial_editor_params: None,
            preview: None,
            icon_cache: None,
            edit_rx: None,
            edit_gen: 0,
            last_slider_change: None,
            modal: None,
            save_as_modal: None,
            pending_error: None,
            pending_rename: None,
            pending_delete: None,
            pending_close: false,
            pending_save_and_exit: None,
            show_help: false,
            show_info: false,
        }
    }

    fn in_editor(&self) -> bool {
        self.editor_state.is_some()
    }

    /// Returns the image to display: editor preview if in editor, else raw image.
    fn view(&self) -> Option<&DecodedImage> {
        self.editor_state
            .as_ref()
            .map(|es| &es.display)
            .or(self.current_image.as_ref())
    }

    fn current_filename(&self) -> Option<String> {
        self.current_path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    }

    // ── Image loading ─────────────────────────────────────────────────────────

    fn open_image(&mut self, path: PathBuf, mode: OpenMode, ctx: &egui::Context) {
        // Serve from preload cache if available — no disk I/O.
        if let Some(cached) = self.preload_cache.remove(&path) {
            self.apply_image(path, cached, mode, ctx);
            ctx.request_repaint();
            return;
        }
        // Deduplicate concurrent loads of the same path.
        if self.loading_path.as_deref() == Some(&path) {
            return;
        }
        self.loading_path = Some(path.clone());
        self.loading_since = Some(Instant::now());

        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match load_image(&path) {
                LoadOutcome::Ok(img) => Msg::Loaded {
                    path,
                    image: img,
                    mode,
                },
                LoadOutcome::WrongExtension(img, ext) => Msg::WrongExtension {
                    path,
                    image: img,
                    mode,
                    correct_ext: ext,
                },
                LoadOutcome::Unsupported(reason) => Msg::Unsupported { path, reason, mode },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn apply_image(
        &mut self,
        path: PathBuf,
        img: DecodedImage,
        mode: OpenMode,
        ctx: &egui::Context,
    ) {
        self.loading_path = None;
        self.loading_since = None;
        self.bad_file = None;
        self.deleting = false;
        self.frame_index = 0;
        self.metadata = None;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;

        // Schedule first animation frame for animated images.
        self.next_frame_at = if img.frames.len() > 1 {
            Some(Instant::now() + Duration::from_millis(img.frame(0).delay_ms as u64))
        } else {
            None
        };

        let dir_changed = self.current_path.as_deref().and_then(|p| p.parent()) != path.parent();

        self.current_image = Some(img.clone());
        self.current_path = Some(path.clone());
        self.image_gen += 1;

        if dir_changed || mode == OpenMode::FirstImage {
            self.navigator.set_current(path.clone());
            self.watch_directory(path.parent().map(|p| p.to_path_buf()), ctx);
        }

        self.preload_neighbour(true, ctx);
        self.preload_neighbour(false, ctx);

        // Load XMP labels synchronously — tiny file, negligible I/O.
        self.labels = self
            .current_path
            .as_deref()
            .map(xmp::read_labels)
            .unwrap_or_default();

        // Load EXIF asynchronously — never blocks navigation.
        let meta_path = path.clone();
        let tx_meta = self.msg_tx.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let meta = exif::read_metadata(&meta_path);
            let _ = tx_meta.send(Msg::MetadataLoaded {
                path: meta_path,
                meta,
            });
            ctx2.request_repaint();
        });
    }

    /// A file that would not open takes the image's place as a card, and
    /// browsing goes on from it: it is current, so the arrows, Delete and the
    /// title all work as for an image. Before, an error modal stopped
    /// everything, and `loading_path` stayed set, so the file could not be
    /// tried again.
    fn show_bad_file(
        &mut self,
        path: PathBuf,
        reason: String,
        mode: OpenMode,
        ctx: &egui::Context,
    ) {
        if self.loading_path.as_ref().is_some_and(|p| *p != path) {
            return; // the user has already moved on to another file
        }
        let dir_changed = self.current_path.as_deref().and_then(|p| p.parent()) != path.parent();
        self.clear_image();
        self.loading_path = None;
        self.loading_since = None;
        self.labels = xmp::read_labels(&path);
        self.current_path = Some(path.clone());
        self.bad_file = Some(reason);
        if dir_changed || mode == OpenMode::FirstImage {
            self.navigator.set_current(path.clone());
            self.watch_directory(path.parent().map(|p| p.to_path_buf()), ctx);
        }
        self.preload_neighbour(true, ctx);
        self.preload_neighbour(false, ctx);
    }

    fn preload_neighbour(&mut self, forward: bool, ctx: &egui::Context) {
        // Peek: next() and prev() move the navigator's position.
        let target = if forward {
            self.navigator.peek_next()
        } else {
            self.navigator.peek_prev()
        };
        let Some(p) = target else { return };
        if self.preload_cache.contains_key(&p) {
            return;
        }
        if self.loading_path.as_deref() == Some(&p) {
            return;
        }

        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let LoadOutcome::Ok(img) = load_image(&p) {
                let _ = tx.send(Msg::Preloaded {
                    path: p,
                    image: img,
                });
                ctx.request_repaint();
            }
        });
    }

    fn watch_directory(&mut self, dir: Option<PathBuf>, ctx: &egui::Context) {
        let Some(dir) = dir else { return };
        if self.watched_dir.as_deref() == Some(&dir) {
            return;
        }
        self.watched_dir = Some(dir.clone());

        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        if let Ok(mut w) = new_debouncer(DIR_WATCH_DEBOUNCE, move |_| {
            let _ = tx.send(Msg::DirectoryChanged);
            ctx.request_repaint();
        }) {
            use notify_debouncer_mini::notify::RecursiveMode;
            let _ = w.watcher().watch(&dir, RecursiveMode::NonRecursive);
            self.dir_watcher = Some(w);
        }
    }

    fn clear_image(&mut self) {
        self.current_image = None;
        self.current_path = None;
        self.frame_index = 0;
        self.next_frame_at = None;
        self.image_gen += 1;
        self.metadata = None;
        self.labels = ImageLabels::default();
        self.bad_file = None;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
    }

    // ── Animation ─────────────────────────────────────────────────────────────

    fn tick_animation(&mut self) -> bool {
        let Some(at) = self.next_frame_at else {
            return false;
        };
        if Instant::now() < at {
            return false;
        }
        let Some(img) = &self.current_image else {
            self.next_frame_at = None;
            return false;
        };
        let n = img.frames.len();
        if n <= 1 {
            self.next_frame_at = None;
            return false;
        }

        self.frame_index = (self.frame_index + 1) % n;
        let delay = img.frame(self.frame_index).delay_ms;
        self.next_frame_at = Some(Instant::now() + Duration::from_millis(delay as u64));
        self.image_gen += 1;
        true
    }

    // ── Message drain ─────────────────────────────────────────────────────────

    fn drain_messages(&mut self, ctx: &egui::Context) -> DrainResult {
        let mut dr = DrainResult::default();
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::Loaded { path, image, mode } => {
                    let (w, h) = (image.width, image.height);
                    self.apply_image(path, image, mode, ctx);
                    if mode == OpenMode::FirstImage {
                        dr.resize_to = Some((w, h));
                    }
                }
                Msg::WrongExtension {
                    path,
                    image,
                    mode,
                    correct_ext,
                } => {
                    let (w, h) = (image.width, image.height);
                    self.apply_image(path.clone(), image, mode, ctx);
                    if mode == OpenMode::FirstImage {
                        dr.resize_to = Some((w, h));
                    }
                    dr.rename_offer = Some((path, correct_ext));
                }
                Msg::Unsupported { path, reason, mode } => {
                    log!("[load] unsupported: {}: {}", path.display(), reason);
                    self.show_bad_file(path, reason, mode, ctx);
                }
                Msg::Preloaded { path, image } => {
                    self.preload_cache.insert(path, image);
                }
                Msg::Deleted { path, result } => {
                    self.deleting = false;
                    match result {
                        Ok(()) => {
                            log!("[delete] moved to the Recycle Bin: {}", path.display());
                            self.navigator.remove(&path);
                            self.preload_cache.remove(&path);
                            self.toast.show("Moved to the Recycle Bin");
                        }
                        // The view has already moved on; the file is still
                        // there and still in the list, so it can be reopened.
                        Err(e) => {
                            dr.error_msg =
                                Some(format!("Could not delete {}\n\n{e}", path.display()))
                        }
                    }
                }
                Msg::MetadataLoaded { path, meta } => {
                    // Only apply if this is still the current image.
                    if self.current_path.as_deref() == Some(&*path) {
                        self.metadata = if meta.is_empty() { None } else { Some(meta) };
                    }
                }
                Msg::DirectoryChanged => {
                    if let Some(dir) = &self.watched_dir {
                        self.navigator.rebuild_current(Some(dir.as_path()));
                    }
                }
            }
        }
        dr
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    fn navigate(&mut self, forward: bool, ctx: &egui::Context) {
        let next = if forward {
            self.navigator.next()
        } else {
            self.navigator.prev()
        };
        if let Some(p) = next {
            self.open_image(p, OpenMode::Navigate, ctx);
            // A slideshow is for looking; the HUD stays down unless the mouse moves.
            if !self.slideshow_active {
                self.hud_since = Some(Instant::now());
            }
        }
    }

    /// Ctrl+O, or Open… in the empty state.
    fn open_dialog(&mut self, ctx: &egui::Context) {
        let mut d = rfd::FileDialog::new()
            .set_title("Open image")
            .add_filter("Images", loader::SUPPORTED_EXTENSIONS);
        if let Some(dir) = self.current_path.as_deref().and_then(Path::parent) {
            d = d.set_directory(dir);
        }
        if let Some(path) = d.pick_file() {
            let mode = if self.current_path.is_none() {
                OpenMode::FirstImage
            } else {
                OpenMode::Navigate
            };
            self.open_image(path, mode, ctx);
            self.hud_since = Some(Instant::now());
        }
    }

    // ── Window ────────────────────────────────────────────────────────────────

    fn toggle_maximize(&mut self, ctx: &egui::Context) {
        if self.maximized {
            if let Some(saved) = self.pre_maximize_size.take() {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(saved));
                if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
                    ctx.send_viewport_cmd(cmd);
                }
            }
            self.maximized = false;
        } else {
            self.pre_maximize_size = Some(ctx.screen_rect().size());
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            self.maximized = true;
        }
    }

    /// Defers the fullscreen toggle to the next `update()` call where it can
    /// be applied correctly via `ViewportCommand`.
    fn toggle_fullscreen(&mut self, _ctx: &egui::Context) {
        self.pending_fullscreen_toggle = true;
    }

    fn start_slideshow(&mut self, ctx: &egui::Context) {
        if self.current_image.is_none() {
            return;
        }
        self.slideshow_active = true;
        self.slideshow_next_at = Some(Instant::now() + SLIDESHOW_INTERVAL);
        // Enter fullscreen for the slideshow experience.
        if !self.fullscreen {
            self.toggle_fullscreen(ctx);
        }
        ctx.request_repaint();
    }

    fn stop_slideshow(&mut self, ctx: &egui::Context) {
        self.slideshow_active = false;
        self.slideshow_next_at = None;
        // Exit fullscreen when slideshow ends.
        if self.fullscreen {
            self.toggle_fullscreen(ctx);
        }
    }

    fn toggle_slideshow(&mut self, ctx: &egui::Context) {
        if self.slideshow_active {
            self.stop_slideshow(ctx);
        } else {
            self.start_slideshow(ctx);
        }
    }

    // ── Editor lifecycle ──────────────────────────────────────────────────────

    fn enter_editor(&mut self, ctx: &egui::Context) {
        if self.editor_state.is_some() {
            return;
        }
        // Clone the image out first — we need &mut self for stop_slideshow
        // and EditorState::new, which conflicts with borrowing current_image.
        let Some(img) = self.current_image.clone() else {
            return;
        };
        if self.slideshow_active {
            self.stop_slideshow(ctx);
        }
        self.editor_state = Some(EditorState::new(&img));
        self.initial_editor_params = Some(EditParams::default());
    }

    /// Returns `true` if the editor was exited or was already closed.
    fn exit_editor(&mut self, quit_intent: bool) -> bool {
        if self.editor_state.is_none() {
            return true;
        }
        // A dialog is already asking something: leave the editor only through
        // its answer. Exiting here discarded the edits "Save changes?" was
        // asking about.
        if self.modal.is_some() {
            return false;
        }
        if self.editor_params_changed() {
            self.modal = Some(ModalDialog::SaveOnExit {
                save_and_quit: quit_intent,
            });
            return false;
        }
        self.do_exit_editor();
        true
    }

    fn do_exit_editor(&mut self) {
        self.editor_state = None;
        self.initial_editor_params = None;
        self.preview = None;
        self.icon_cache = None;
        self.edit_rx = None;
        self.last_slider_change = None;
        // Force the GPU texture to be re-uploaded from current_image.
        // The editor's async pipeline may have left a modified image in the
        // texture slot — incrementing image_gen triggers a fresh upload from
        // the unedited current_image on the next frame.
        self.image_gen += 1;
    }

    fn editor_params_changed(&self) -> bool {
        match (&self.editor_state, &self.initial_editor_params) {
            (Some(es), Some(init)) => !params_equal(es.undo.current(), init),
            _ => false,
        }
    }

    fn editor_params_mut(&mut self, f: impl FnOnce(&mut EditParams)) -> EditParams {
        debug_assert!(
            self.in_editor(),
            "editor_params_mut called outside editor mode"
        );
        let mut p = self
            .editor_state
            .as_ref()
            .map(|es| es.undo.current().clone())
            .unwrap_or_default();
        f(&mut p);
        p
    }

    fn editor_undo(&mut self) {
        self.editor_step(|u| u.undo());
    }
    fn editor_redo(&mut self) {
        self.editor_step(|u| u.redo());
    }

    fn editor_step(&mut self, f: impl FnOnce(&mut editor::UndoStack)) {
        if let Some(es) = &mut self.editor_state {
            f(&mut es.undo);
            es.dirty = true;
        }
        if let Some(p) = self
            .editor_state
            .as_ref()
            .map(|es| es.undo.current().clone())
        {
            self.launch_async_edit(p);
        }
    }

    /// Commit params, cancel debounce, and immediately launch async re-encode.
    fn editor_commit_and_sync(&mut self, p: EditParams) {
        self.last_slider_change = None;
        if let Some(es) = &mut self.editor_state {
            es.commit(p.clone());
        }
        self.launch_async_edit(p);
    }

    fn launch_async_edit(&mut self, params: EditParams) {
        let Some(orig) = self.current_image.clone() else {
            return;
        };
        self.edit_gen += 1;
        let gen = self.edit_gen;
        let (tx, rx) = bounded(1);
        self.edit_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send((gen, apply_edits(&orig, &params)));
        });
    }

    fn poll_edit_result(&mut self) -> bool {
        let Some(rx) = &self.edit_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok((gen, img)) => {
                self.edit_rx = None;
                if gen == self.edit_gen {
                    if let Some(es) = &mut self.editor_state {
                        es.display = img;
                        es.dirty = false;
                        self.image_gen += 1;
                    }
                    return true;
                }
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => false,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.edit_rx = None;
                false
            }
        }
    }

    // ── Panel action dispatch ─────────────────────────────────────────────────

    fn apply_panel_action(&mut self, action: PanelAction, ctx: &egui::Context) {
        // Any explicit action cancels pending debounced slider re-encode.
        if action.has_explicit_edit() {
            self.last_slider_change = None;
        }

        if action.save {
            log!("[action] Save");
            self.save_inplace();
            return;
        }
        if action.save_as {
            log!("[action] SaveAs");
            self.save_as_returning(ctx);
            return;
        }
        if action.undo {
            log!("[action] Undo");
            self.editor_undo();
            return;
        }
        if action.redo {
            log!("[action] Redo");
            self.editor_redo();
            return;
        }

        // Geometry actions: dispatch without heap allocation.
        enum Geom {
            ResetAll,
            RotCw,
            RotCcw,
            FlipH,
            FlipV,
        }
        let geom = if action.reset_all {
            Some(Geom::ResetAll)
        } else if action.rot_cw {
            Some(Geom::RotCw)
        } else if action.rot_ccw {
            Some(Geom::RotCcw)
        } else if action.flip_h {
            Some(Geom::FlipH)
        } else if action.flip_v {
            Some(Geom::FlipV)
        } else {
            None
        };

        if let Some(g) = geom {
            let p = match g {
                Geom::ResetAll => {
                    log!("[action] ResetAll");
                    EditParams::default()
                }
                Geom::RotCw => {
                    log!("[action] RotCw");
                    self.editor_params_mut(|p| p.rotation = p.rotation.cw())
                }
                Geom::RotCcw => {
                    log!("[action] RotCcw");
                    self.editor_params_mut(|p| p.rotation = p.rotation.ccw())
                }
                Geom::FlipH => {
                    log!("[action] FlipH");
                    self.editor_params_mut(|p| p.flip_h = !p.flip_h)
                }
                Geom::FlipV => {
                    log!("[action] FlipV");
                    self.editor_params_mut(|p| p.flip_v = !p.flip_v)
                }
            };
            self.editor_commit_and_sync(p);
            return;
        }

        // Slider / preset change: debounce before re-encoding. A drag's later
        // frames replace the step its first frame made.
        if let Some(new_p) = action.new_params {
            log!("[action] ParamChange");
            if let Some(es) = &mut self.editor_state {
                es.commit(new_p);
            }
            self.last_slider_change = Some(Instant::now());
        } else if let Some(p) = action.drag_params {
            if let Some(es) = &mut self.editor_state {
                *es.undo.current_mut() = p;
                es.dirty = true;
            }
            self.last_slider_change = Some(Instant::now());
        }
    }

    // ── Save / delete ─────────────────────────────────────────────────────────

    /// Returns whether the file now holds the edits; a failure is reported
    /// through `pending_error`.
    fn save_inplace(&mut self) -> bool {
        let Some(path) = self.current_path.clone() else {
            return false;
        };
        let Some(es) = &self.editor_state else {
            return false;
        };
        let params = es.undo.current().clone();
        let Some(display) = self.pixels_to_save() else {
            return false;
        };

        // A JPEG that was only rotated or flipped keeps its compressed data:
        // the change goes into the EXIF orientation (see orientation.rs).
        let saved = if is_pure_geometry(&params) && is_jpeg_path(&path) {
            orientation::save_geometry(&path, &params).map(|wrote| {
                if wrote {
                    log!("[save] EXIF orientation written; image data untouched");
                    "Saved \u{2014} orientation stored in EXIF, pixels untouched"
                } else {
                    log!("[save] orientation already as shown; file not written");
                    "Nothing to save \u{2014} the file already looks like this"
                }
            })
        } else {
            save::to_path(
                &display,
                self.frame_index,
                &path,
                95,
                None,
                &save::Metadata::read(&path),
            )
            .map(|()| "Saved")
        };
        match saved {
            Ok(said) => self.toast.show(said),
            Err(e) => {
                self.pending_error = Some(format!("Save failed:\n{e}"));
                return false;
            }
        }
        // The file now holds what is on screen, so it becomes the image the
        // next edit starts from, as in commit_save_as. Without this the edit
        // just saved would be applied to the pre-save pixels and lost.
        self.current_image = Some(display);
        self.image_gen += 1;
        self.reset_editor_after_save();
        true
    }

    /// Opens the custom Save As modal; actual write happens on confirm.
    fn save_as_returning(&mut self, _ctx: &egui::Context) -> bool {
        if self.editor_state.is_none() {
            return false;
        }
        self.save_as_modal = Some(self.default_save_as_modal());
        true
    }

    fn default_save_as_modal(&self) -> SaveAsModal {
        let path = self.current_path.as_deref();
        let stem = path
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}_edited", s.to_string_lossy()))
            .unwrap_or_else(|| "edited".into());
        let format = path
            .map(OutputFormat::from_path)
            .unwrap_or(OutputFormat::Png);
        let directory = path
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        SaveAsModal {
            stem,
            format,
            directory,
            quality: 95,
            max_dim: None,
            preset: ExportPreset::Custom,
            replace: None,
        }
    }

    fn commit_save_as(&mut self, dest: PathBuf, quality: u8, max_dim: Option<u32>) {
        let Some(display) = self.pixels_to_save() else {
            return;
        };
        let meta = self
            .current_path
            .as_deref()
            .map(save::Metadata::read)
            .unwrap_or_default();
        if let Err(e) = save::to_path(&display, self.frame_index, &dest, quality, max_dim, &meta) {
            self.pending_error = Some(format!("Save failed:\n{e}"));
            return;
        }
        if let Some(name) = dest.file_name() {
            self.toast
                .show(format!("Saved as {}", name.to_string_lossy()));
        }
        self.current_path = Some(dest.clone());
        self.current_image = Some(display);
        self.navigator.set_current(dest);
        self.image_gen += 1;
        self.reset_editor_after_save();
    }

    /// The committed edits applied to the source, rendered now. Not
    /// `es.display`: that is the last frame the screen showed, which lags the
    /// params during the slider debounce and shows a preset while one is only
    /// hovered.
    fn pixels_to_save(&self) -> Option<DecodedImage> {
        let es = self.editor_state.as_ref()?;
        let src = self.current_image.as_ref()?;
        Some(editor::apply_edits(src, es.undo.current()))
    }

    fn reset_editor_after_save(&mut self) {
        if let Some(es) = &mut self.editor_state {
            es.undo.push(EditParams::default());
            es.dirty = false;
        }
        self.initial_editor_params = Some(EditParams::default());
    }

    fn delete_current(&mut self) {
        let Some(path) = &self.current_path else {
            return;
        };
        if self.deleting {
            return;
        }
        self.modal = Some(ModalDialog::ConfirmDelete { path: path.clone() });
    }

    /// Move `path`, the file the dialog named, to the Recycle Bin.
    fn execute_delete(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.deleting {
            return;
        }
        self.deleting = true;

        // Leave the image only when it is the one going away.
        if self.current_path.as_ref() == Some(&path) {
            if self.in_editor() {
                self.do_exit_editor();
            }

            // Navigate away before deleting so display doesn't go blank.
            let next = self
                .navigator
                .next()
                .filter(|p| p != &path)
                .or_else(|| self.navigator.prev().filter(|p| p != &path));
            if let Some(np) = next {
                self.open_image(np, OpenMode::Navigate, ctx);
            } else {
                self.clear_image();
            }
        }

        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = move_to_trash(&path);
            let _ = tx.send(Msg::Deleted { path, result });
            ctx.request_repaint();
        });
    }
} // end impl App

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.apply_deferred_viewport(ctx, frame);
        self.guard_close(ctx);
        self.process_background_messages(ctx);
        self.tick_and_schedule_animation(ctx);
        self.drive_async_edit(ctx);
        self.upload_current_frame(ctx, frame);
        self.draw_ui(ctx);
        self.process_keyboard(ctx);
        self.sync_title(ctx);
    }

    fn on_exit(&mut self) {
        // Background threads hold Sender/Receiver clones and will exit naturally.
    }
}

impl App {
    // ── update() sub-steps (called in order) ──────────────────────────────────

    /// Apply deferred viewport commands that must run at the start of a frame.
    fn apply_deferred_viewport(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.pending_fullscreen_toggle {
            self.pending_fullscreen_toggle = false;
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        self.apply_pending_actions(ctx);
    }

    /// Carry out what a dialog answered last frame.
    fn apply_pending_actions(&mut self, ctx: &egui::Context) {
        if let Some(quit_after) = self.pending_save_and_exit.take() {
            // A failed save keeps the editor, the edits and the window open;
            // its error dialog is next frame's modal.
            if self.save_inplace() {
                self.do_exit_editor();
                if quit_after {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if let Some(path) = self.pending_delete.take() {
            self.execute_delete(path, ctx);
        }
        if std::mem::take(&mut self.pending_close) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The window's close button, Alt+F4 and every other close ask first when
    /// there are unsaved edits, as Q does: they closed and dropped the edits.
    fn guard_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || !self.editor_params_changed() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        // An open dialog is answered first; the next close asks again.
        if self.modal.is_none() {
            self.save_as_modal = None;
            self.modal = Some(ModalDialog::SaveOnExit {
                save_and_quit: true,
            });
        }
    }

    /// Drain all queued background messages and act on results.
    fn process_background_messages(&mut self, ctx: &egui::Context) {
        let dr = self.drain_messages(ctx);

        // Resize the window exactly once — on the first image opened at launch.
        if let Some((w, h)) = dr.resize_to {
            if !self.has_shown_first_image {
                self.has_shown_first_image = true;
                let (cw, ch) = clamp_to_work_area(w, h, ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    cw as f32, ch as f32,
                )));
                if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
                    ctx.send_viewport_cmd(cmd);
                }
            }
        }
        if let Some(msg) = dr.error_msg {
            self.pending_error = Some(msg);
        }
        if let Some(offer) = dr.rename_offer {
            self.pending_rename = Some(offer);
        }
    }

    /// Advance animation by one frame if the timer has elapsed.
    fn tick_and_schedule_animation(&mut self, ctx: &egui::Context) {
        if self.tick_animation() {
            ctx.request_repaint();
        }
        if let Some(at) = self.next_frame_at {
            ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
        }

        // Advance slideshow when the timer fires.
        if self.slideshow_active {
            if let Some(at) = self.slideshow_next_at {
                if Instant::now() >= at {
                    self.navigate(true, ctx);
                    self.slideshow_next_at = Some(Instant::now() + SLIDESHOW_INTERVAL);
                }
                ctx.request_repaint_after(
                    self.slideshow_next_at
                        .unwrap()
                        .saturating_duration_since(Instant::now()),
                );
            }
        }
    }

    /// Drive the debounced slider pipeline and poll for completed edits.
    fn drive_async_edit(&mut self, ctx: &egui::Context) {
        if self
            .last_slider_change
            .is_some_and(|t| t.elapsed().as_millis() >= SLIDER_DEBOUNCE_MS)
        {
            self.last_slider_change = None;
            if let Some(p) = self
                .editor_state
                .as_ref()
                .map(|es| es.undo.current().clone())
            {
                log!("[debounce] firing async edit after slider idle");
                self.launch_async_edit(p);
            }
        } else if self.last_slider_change.is_some() || self.edit_rx.is_some() {
            ctx.request_repaint();
        }
        if self.poll_edit_result() {
            ctx.request_repaint();
        }
    }

    /// Upload pixel data to the GPU if the image generation or frame changed.
    fn upload_current_frame(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.image_gen == self.last_uploaded_gen && self.frame_index == self.last_uploaded_frame
        {
            return;
        }

        self.last_uploaded_gen = self.image_gen;
        self.last_uploaded_frame = self.frame_index;

        // Clone pixels before the mutable borrow on image_texture.
        // Clone pixels before the mutable borrow on image_texture.
        // Pass canvas size so upload can pre-downsample for high-quality minification.
        let canvas_size = ctx.screen_rect().size();
        let upload = self.view().map(|img| {
            let fd = img.frame(self.frame_index);
            (fd.pixels.clone(), img.width, img.height)
        });
        if let Some((pixels, w, h)) = upload {
            let t = Instant::now();
            upload_image_texture(
                ctx,
                frame,
                &pixels,
                w,
                h,
                canvas_size,
                &mut self.image_texture,
            );
            let ms = t.elapsed().as_millis();
            if ms > 2 {
                log!("[upload] {}×{} {}ms", w, h, ms);
            }
        }
    }

    /// Build the egui UI for this frame.
    fn draw_ui(&mut self, ctx: &egui::Context) {
        ui::apply_panel_style(ctx);
        self.promote_pending_modals(ctx);
        self.ensure_editor_caches(ctx);

        // Modals overlay everything — render before panels.
        let clicked = self
            .modal
            .as_ref()
            .and_then(|modal| draw_modal(ctx, modal, self.icon_cache.as_ref()));
        if let Some(action) = clicked {
            apply_modal_action(self, action);
            self.modal = None;
        }
        draw_save_as_modal(ctx, self);
        if self.show_help {
            draw_help_modal(ctx, self.in_editor(), &mut self.show_help);
        }

        // SidePanel must precede CentralPanel so egui allocates its input region first.
        let pending_action = self.draw_editor_panel(ctx);

        self.draw_canvas(ctx);
        if self.show_info {
            ui::draw_info_card(ctx, self.metadata.as_ref());
        }

        if let Some(action) = pending_action {
            self.apply_panel_action(action, ctx);
            ctx.request_repaint();
        }
    }

    /// Promote queued error/rename messages into modal dialogs.
    fn promote_pending_modals(&mut self, _ctx: &egui::Context) {
        if let Some(msg) = self.pending_error.take() {
            self.modal = Some(ModalDialog::Error { message: msg });
        }
        if let Some((old_path, correct_ext)) = self.pending_rename.take() {
            if let Some(new_path) = offer_rename_if_writable(&old_path, &correct_ext) {
                let old_ext = old_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                self.modal = Some(ModalDialog::OfferRename {
                    old_path,
                    new_path,
                    old_ext,
                    new_ext: correct_ext,
                });
            }
        }
    }

    /// Show the hovered preset chip on the canvas, and the edit itself again
    /// when the pointer leaves it. Only `display` changes: the undo stack and
    /// what a save writes (`edited_pixels`) never see a preview.
    fn preview_preset(&mut self, hover: Option<editor::Preset>, current: &EditParams) {
        if hover == self.preview {
            return;
        }
        self.preview = hover;
        let mut p = current.clone();
        if let Some(preset) = hover {
            p.preset = preset;
        }
        self.launch_async_edit(p);
    }

    /// Lazily initialise the icon cache when the editor is open.
    fn ensure_editor_caches(&mut self, ctx: &egui::Context) {
        if !self.in_editor() {
            return;
        }
        let px = ui::icon_px(ctx);
        if self.icon_cache.as_ref().map(|i| i.px) != Some(px) {
            self.icon_cache = Some(Icons::load(ctx, px));
        }
    }

    /// Draw the editor side panel; returns any pending action for this frame.
    fn draw_editor_panel(&mut self, ctx: &egui::Context) -> Option<PanelAction> {
        if !self.in_editor() {
            return None;
        }
        let params = self
            .editor_state
            .as_ref()
            .map(|es| es.undo.current().clone())
            .unwrap_or_default();
        let can_undo = self
            .editor_state
            .as_ref()
            .map(|es| es.undo.can_undo())
            .unwrap_or(false);
        let can_redo = self
            .editor_state
            .as_ref()
            .map(|es| es.undo.can_redo())
            .unwrap_or(false);
        let action = draw_panel(
            ctx,
            PanelInput {
                params: &params,
                can_undo,
                can_redo,
                dirty: self.editor_params_changed(),
                icons: self.icon_cache.as_ref().unwrap(),
            },
        );
        self.preview_preset(action.hover_preset, &params);
        if action.has_any() {
            Some(action)
        } else {
            None
        }
    }

    /// Draw the canvas (CentralPanel): image, scroll navigation, drag-and-drop.
    fn draw_canvas(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(ui::tok::CANVAS))
            .show(ctx, |ui| {
                let canvas = ui.available_rect_before_wrap();
                self.draw_canvas_image(ui);
                self.draw_viewer_overlays(ui, canvas);
                self.handle_canvas_scroll(ui, ctx);
                self.handle_canvas_drop(ui, ctx);
            });
    }

    fn draw_canvas_image(&mut self, ui: &mut egui::Ui) {
        // The texture outlives the image it was made from (it is reused for
        // the next one), so with no image there is nothing to draw.
        if self.view().is_none() {
            return;
        }
        let Some(it) = &self.image_texture else {
            return;
        };
        let canvas = ui.available_rect_before_wrap();
        // Allocate the full canvas first for reliable click/drag sense.
        let canvas_resp = ui.allocate_rect(canvas, egui::Sense::click_and_drag());

        // Fit image to canvas, then apply view zoom.
        let fit = (canvas.width() / it.width as f32).min(canvas.height() / it.height as f32);
        let dw = it.width as f32 * fit * self.zoom;
        let dh = it.height as f32 * fit * self.zoom;

        // Centre + apply pan offset. In editor mode, zoom and pan are active.
        let cx = canvas.min.x + (canvas.width() - dw) * 0.5;
        let cy = canvas.min.y + (canvas.height() - dh) * 0.5;
        let (ox, oy) = if self.in_editor() {
            // Clamp pan so the image cannot be dragged entirely off the canvas.
            // Allow panning up to (image_size - canvas_size) / 2 in each direction,
            // so at least half the image is always visible.
            let max_pan_x = ((dw - canvas.width()) * 0.5).max(0.0);
            let max_pan_y = ((dh - canvas.height()) * 0.5).max(0.0);
            let px = self.pan.x.clamp(-max_pan_x, max_pan_x);
            let py = self.pan.y.clamp(-max_pan_y, max_pan_y);
            // Also prevent panning into the right panel: cap ox so the image
            // left edge never goes past canvas.max.x (the panel boundary).
            let ox = (cx + px).min(canvas.max.x - 1.0);
            let oy = cy + py;
            (ox, oy)
        } else {
            (cx, cy)
        };

        let img_rect = egui::Rect::from_min_size(egui::pos2(ox, oy), egui::vec2(dw, dh));

        egui::Image::new(egui::load::SizedTexture::new(
            it.texture_id,
            egui::vec2(it.width as f32, it.height as f32),
        ))
        .fit_to_exact_size(egui::vec2(dw, dh))
        .paint_at(ui, img_rect);

        // ── Guide overlays (editor mode only) ─────────────────────────────────
        if self.in_editor() {
            draw_guides(ui, img_rect, self.guides);
        }

        // ── Pan (drag in editor mode) ──────────────────────────────────────────
        if self.in_editor() && canvas_resp.dragged() {
            self.pan += canvas_resp.drag_delta();
        }

        // ── Double-click: toggle maximize (when not dragging) ─────────────────
        if canvas_resp.double_clicked() {
            self.toggle_maximize(ui.ctx());
        }
    }

    fn handle_canvas_scroll(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (scroll_y, ctrl_held) = ui.input_mut(|i| {
            let dir = i.events.iter().find_map(|ev| {
                if let egui::Event::Scroll(d) = ev {
                    if d.y.abs() > d.x.abs() && d.y != 0.0 {
                        return Some(d.y);
                    }
                }
                None
            });
            i.events.retain(|ev| !matches!(ev, egui::Event::Scroll(_)));
            (dir, i.modifiers.ctrl)
        });

        let Some(dy) = scroll_y else { return };

        if self.in_editor() && ctrl_held {
            // Ctrl+scroll → zoom. Each notch changes zoom by 15%.
            let factor = if dy > 0.0 { 1.15_f32 } else { 1.0 / 1.15 };
            self.zoom = (self.zoom * factor).clamp(0.1, 10.0);
        } else if !self.in_editor() {
            // Viewer mode → navigate images.
            self.navigate(dy < 0.0, ctx);
        }
    }

    fn handle_canvas_drop(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let dropped = ui.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped {
            if !self.in_editor() {
                self.open_image(path, OpenMode::Navigate, ctx);
            }
        }
    }

    /// Everything item 6 puts over the canvas: the bad-file card or the empty
    /// state where there is no image, the loading bar, the HUD and the toast.
    /// Each fade asks for exactly the frames it needs.
    fn draw_viewer_overlays(&mut self, ui: &mut egui::Ui, canvas: egui::Rect) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let editor = self.in_editor();

        if !editor {
            if let (Some(reason), Some(path)) = (&self.bad_file, &self.current_path) {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                hud::draw_bad_file_card(ui.painter(), canvas, &name, reason);
            } else if self.current_path.is_none()
                && self.loading_path.is_none()
                && hud::draw_empty_state(ui, canvas)
            {
                self.open_dialog(&ctx);
            }
            if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
                hud::draw_drop_target(ui.painter(), canvas);
            }
        }

        if let Some(since) = self.loading_since.filter(|_| self.loading_path.is_some()) {
            if hud::loading_bar_due(since, now) {
                hud::draw_loading_bar(ui.painter(), canvas, now - since);
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(hud::LOADING_BAR_AFTER);
            }
        }

        if !editor {
            let moved = ctx.input(|i| {
                i.pointer.delta() != egui::Vec2::ZERO
                    && i.pointer.hover_pos().is_some_and(|p| canvas.contains(p))
            });
            if moved {
                self.hud_since = Some(now);
            }
            if let (Some(since), Some(path)) = (self.hud_since, &self.current_path) {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let info = hud::HudInfo {
                    name: &name,
                    position: self.navigator.position_of(path),
                    dims: self.current_image.as_ref().map(|i| (i.width, i.height)),
                    labels: &self.labels,
                };
                let alpha = hud::fade_alpha(since, hud::HUD_HOLD, now);
                hud::draw_hud(ui.painter(), canvas, &info, alpha);
                match hud::fade_repaint(since, hud::HUD_HOLD, now) {
                    Some(after) => ctx.request_repaint_after(after),
                    None => self.hud_since = None,
                }
            }
        }

        if let Some((text, since)) = &self.toast.current {
            let alpha = hud::fade_alpha(*since, hud::TOAST_HOLD, now);
            hud::draw_toast(ui.painter(), canvas, text, alpha);
            match hud::fade_repaint(*since, hud::TOAST_HOLD, now) {
                Some(after) => ctx.request_repaint_after(after),
                None => self.toast.current = None,
            }
        }
    }

    /// Handle the app's keyboard shortcuts for this frame.
    ///
    /// While a dialog, Save As, the help overlay or a text field is open, the
    /// keyboard is theirs and this does nothing. Before, a letter typed into
    /// the Save As filename also ran its shortcut (Q started a quit), and the
    /// arrow keys moved to another image under an open delete dialog.
    fn process_keyboard(&mut self, ctx: &egui::Context) {
        if self.modal.is_some()
            || self.save_as_modal.is_some()
            || self.show_help
            || ctx.wants_keyboard_input()
        {
            return;
        }

        // The mode is fixed for the frame, so E opening the editor does not
        // also count as the editor's E.
        let editor = self.in_editor();
        for binding in keymap::KEYMAP.iter().filter(|b| b.mode.includes(editor)) {
            for &(mods, key) in binding.keys {
                if ctx.input_mut(|i| consume_shortcut(i, mods, key)) {
                    self.run_action(binding.action, key, ctx);
                }
            }
        }
    }

    fn run_action(&mut self, action: keymap::Action, key: egui::Key, ctx: &egui::Context) {
        use keymap::Action;
        match action {
            Action::Next => self.navigate(true, ctx),
            Action::Prev => self.navigate(false, ctx),
            Action::Rate => {
                if let Some(path) = &self.current_path {
                    self.labels.rating = keymap::digit(key);
                    match xmp::write_labels(path, &self.labels) {
                        Ok(()) => self.toast.show(hud::rating_toast(self.labels.rating)),
                        Err(e) => {
                            self.pending_error = Some(format!("Could not save the rating:\n{e}"));
                            self.labels = xmp::read_labels(path);
                        }
                    }
                }
            }
            Action::CycleLabel => {
                if let Some(path) = &self.current_path {
                    self.labels.colour = match self.labels.colour {
                        ColourLabel::None => ColourLabel::Red,
                        ColourLabel::Red => ColourLabel::Yellow,
                        ColourLabel::Yellow => ColourLabel::Green,
                        ColourLabel::Green => ColourLabel::Blue,
                        ColourLabel::Blue => ColourLabel::Purple,
                        ColourLabel::Purple => ColourLabel::None,
                    };
                    match xmp::write_labels(path, &self.labels) {
                        Ok(()) => self.toast.show(hud::label_toast(self.labels.colour)),
                        Err(e) => {
                            self.pending_error =
                                Some(format!("Could not save the colour label:\n{e}"));
                            self.labels = xmp::read_labels(path);
                        }
                    }
                }
            }
            Action::EnterEditor => {
                if self.current_image.is_none() {
                    // No pixels, nothing to edit; say so rather than ignore E.
                    self.toast.show(if self.bad_file.is_some() {
                        "This file can\u{2019}t be opened, so it can\u{2019}t be edited"
                    } else {
                        "Open an image to edit it"
                    });
                }
                self.enter_editor(ctx);
            }
            Action::Open => self.open_dialog(ctx),
            Action::LeaveEditor => {
                self.exit_editor(false);
            }
            Action::Slideshow => self.toggle_slideshow(ctx),
            Action::Back => {
                // Stopping the slideshow also leaves fullscreen.
                if self.slideshow_active {
                    self.stop_slideshow(ctx);
                } else if self.fullscreen {
                    self.toggle_fullscreen(ctx);
                }
            }
            Action::Fullscreen => {
                if self.slideshow_active {
                    self.stop_slideshow(ctx);
                } else {
                    self.toggle_fullscreen(ctx);
                }
            }
            Action::Help => self.show_help = !self.show_help,
            Action::Info => self.show_info = !self.show_info,
            Action::Delete => self.delete_current(),
            Action::Quit => {
                // In the editor, unsaved edits ask first.
                if !self.in_editor() || self.exit_editor(true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Undo => self.editor_undo(),
            Action::Redo => self.editor_redo(),
            Action::Save => {
                self.save_inplace();
            }
            Action::SaveAs => {
                self.save_as_returning(ctx);
            }
            Action::CycleGuides => self.guides = self.guides.next(),
            Action::ResetView => {
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;
            }
        }
    }

    /// Keep the window title on what is shown: `● name — 12 / 129 — Patina`.
    /// It used to change only after a save, so navigating left the old name.
    fn sync_title(&mut self, ctx: &egui::Context) {
        let title = window_title(
            self.current_filename().as_deref(),
            self.current_path
                .as_deref()
                .and_then(|p| self.navigator.position_of(p)),
            self.editor_params_changed(),
        );
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

/// The window title: the file, its place in the folder when there is more
/// than one, and ● while there are unsaved edits.
fn window_title(name: Option<&str>, position: Option<(usize, usize)>, dirty: bool) -> String {
    let Some(name) = name else {
        return APP_NAME.to_string();
    };
    let dot = if dirty { "● " } else { "" };
    match position {
        Some((i, n)) if n > 1 => format!("{dot}{name} — {i} / {n} — {APP_NAME}"),
        _ => format!("{dot}{name} — {APP_NAME}"),
    }
}

/// Consume a press of `key` made with `mods`. A Ctrl shortcut matches its
/// modifiers exactly: egui's `consume_key` ignores an extra Shift, so
/// Ctrl+Shift+S also matched Ctrl+S and saved over the original instead of
/// opening Save As, and Ctrl+Shift+Z undid instead of redoing. A plain key
/// still ignores Shift, which some layouts need for digits (AZERTY), but not
/// Ctrl or Alt.
fn consume_shortcut(i: &mut egui::InputState, mods: egui::Modifiers, key: egui::Key) -> bool {
    let fits = |pressed: &egui::Modifiers| {
        if mods.command || mods.ctrl {
            pressed.matches_exact(mods)
        } else {
            !pressed.alt && pressed.matches_logically(mods)
        }
    };
    let mut hit = false;
    i.events.retain(|e| {
        let this = matches!(e, egui::Event::Key { key: k, modifiers, pressed: true, .. }
                               if *k == key && fits(modifiers));
        hit |= this;
        !this
    });
    hit
}

// ── Help modal ────────────────────────────────────────────────────────────────

fn draw_help_modal(ctx: &egui::Context, in_editor: bool, show: &mut bool) {
    use ui::tok::{ACCENT, BORDER, SURFACE, TEXT_1, TEXT_2, TEXT_3};

    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("help_dim"),
    ))
    .rect_filled(ctx.screen_rect(), 0.0, ui::tok::SCRIM);

    egui::Window::new("help_modal")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::none()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(24.0),
        )
        .show(ctx, |ui| {
            ui.set_width(480.0);
            ui.visuals_mut().override_text_color = None;
            ui.visuals_mut().widgets.noninteractive.fg_stroke.color = TEXT_1;

            ui.label(
                egui::RichText::new("Keyboard Shortcuts")
                    .color(TEXT_1)
                    .font(ui::title_font(ui::TITLE)),
            );
            ui.add_space(12.0);

            // Two-column layout: key | description
            let row = |ui: &mut egui::Ui, key: &str, desc: &str| {
                ui.horizontal(|ui| {
                    let (_, kr) =
                        ui.allocate_exact_size(egui::vec2(170.0, 20.0), egui::Sense::hover());
                    ui.painter().text(
                        kr.rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        key,
                        egui::FontId::monospace(12.0),
                        ACCENT,
                    );
                    ui.label(egui::RichText::new(desc).color(TEXT_1).size(12.0));
                });
            };
            let section = |ui: &mut egui::Ui, title: &str| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(title).color(TEXT_3).size(ui::CAPTION));
                ui.add_space(2.0);
            };

            // The mode on screen first; both lists are always there, so the
            // help is a reference, not just a list of what works right now.
            let order = if in_editor {
                [
                    (keymap::Mode::Editor, "EDITOR"),
                    (keymap::Mode::Viewer, "VIEWER"),
                ]
            } else {
                [
                    (keymap::Mode::Viewer, "VIEWER"),
                    (keymap::Mode::Editor, "EDITOR"),
                ]
            };
            for (mode, title) in order {
                section(ui, title);
                for b in keymap::KEYMAP.iter().filter(|b| b.mode == mode) {
                    row(ui, &keymap::keys_label(b.keys), b.what);
                }
                for &(_, gesture, what) in keymap::MOUSE_HELP.iter().filter(|m| m.0 == mode) {
                    row(ui, gesture, what);
                }
            }
            section(ui, "EVERYWHERE");
            for b in keymap::KEYMAP
                .iter()
                .filter(|b| b.mode == keymap::Mode::Both)
            {
                row(ui, &keymap::keys_label(b.keys), b.what);
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("H or Esc to close")
                    .color(TEXT_2)
                    .size(11.0),
            );
        });

    if ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
            || i.consume_key(egui::Modifiers::NONE, egui::Key::H)
    }) {
        *show = false;
    }
}

// ── GPU texture upload ────────────────────────────────────────────────────────

/// Upload RGBA pixels to a wgpu texture registered with egui.
///
/// Reuses the existing texture when dimensions match (animated frames / live edits).
/// Creates and registers a new texture when dimensions change.
/// The `TextureId` is stable until dimensions change — egui never reads the pixels.
// ── Guide overlays ────────────────────────────────────────────────────────────

/// Draw composition guides (rule of thirds or golden ratio) over the image rect.
/// Draw a 5-pointed star centred at `centre` with outer radius `r_out` and inner `r_in`.
fn draw_star(p: &egui::Painter, centre: egui::Pos2, r_out: f32, r_in: f32, color: egui::Color32) {
    use std::f32::consts::PI;
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let angle = PI * i as f32 / 5.0 - PI / 2.0;
        let r = if i % 2 == 0 { r_out } else { r_in };
        pts.push(egui::pos2(
            centre.x + r * angle.cos(),
            centre.y + r * angle.sin(),
        ));
    }
    p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

fn draw_guides(ui: &egui::Ui, r: egui::Rect, guides: Guides) {
    const LINE_COLOR: egui::Color32 = ui::tok::GUIDE;
    const STROKE: egui::Stroke = egui::Stroke {
        width: 1.0,
        color: LINE_COLOR,
    };
    let p = ui.painter();

    let divisions: &[f32] = match guides {
        Guides::Off => return,
        // Rule of thirds: divide into equal thirds
        Guides::Thirds => &[1.0 / 3.0, 2.0 / 3.0],
        // Golden ratio: phi-based divisions (~0.382 / 0.618)
        Guides::Golden => &[0.382, 0.618],
    };

    for &t in divisions {
        // Vertical lines
        let x = r.min.x + r.width() * t;
        p.line_segment([egui::pos2(x, r.min.y), egui::pos2(x, r.max.y)], STROKE);
        // Horizontal lines
        let y = r.min.y + r.height() * t;
        p.line_segment([egui::pos2(r.min.x, y), egui::pos2(r.max.x, y)], STROKE);
    }
}

/// Pre-downsample `pixels` with Lanczos3 when `canvas_sz` would display the image
/// at less than 50% of its native size. Returns the (possibly resampled) buffer
/// and the output dimensions. Caller owns the buffer.
///
/// This eliminates the aliasing that GPU bilinear filtering produces on large
/// images shown at small display sizes. The 50% threshold and 2× oversampling
/// margin balance quality against CPU cost.
fn lanczos_if_needed(pixels: &[u8], w: u32, h: u32, canvas_sz: egui::Vec2) -> (Vec<u8>, u32, u32) {
    let fit = (canvas_sz.x / w as f32).min(canvas_sz.y / h as f32);
    if fit >= 0.5 {
        return (pixels.to_vec(), w, h);
    }

    // Target: 2× the display size for a quality margin above 1:1.
    let target_w = ((w as f32 * fit * 2.0) as u32).max(1);
    let target_h = ((h as f32 * fit * 2.0) as u32).max(1);

    let Some(buf) = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(w, h, pixels.to_vec())
    else {
        return (pixels.to_vec(), w, h);
    };

    let resized = image::imageops::resize(
        &buf,
        target_w,
        target_h,
        image::imageops::FilterType::Lanczos3,
    );
    let (rw, rh) = resized.dimensions();
    log!(
        "[upload] Lanczos {}×{} → {}×{} (fit={:.2})",
        w,
        h,
        rw,
        rh,
        fit
    );
    (resized.into_raw(), rw, rh)
}

fn image_copy(texture: &wgpu::Texture) -> wgpu::ImageCopyTexture<'_> {
    wgpu::ImageCopyTexture {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    }
}

fn upload_image_texture(
    _ctx: &egui::Context,
    frame: &mut eframe::Frame,
    pixels: &[u8],
    w: u32,
    h: u32,
    canvas_sz: egui::Vec2,
    slot: &mut Option<ImageTexture>,
) {
    if w == 0 || h == 0 {
        return;
    }
    let Some(rs) = frame.wgpu_render_state() else {
        ::log::error!("[upload] wgpu render state unavailable");
        return;
    };

    // High-quality downscale: pre-resample with Lanczos3 when the image is displayed
    // at <50% of native size, eliminating GPU linear-filter aliasing.
    let (downsampled, w, h) = lanczos_if_needed(pixels, w, h, canvas_sz);
    let pixels = downsampled.as_slice();

    let extent = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let layout = wgpu::ImageDataLayout {
        offset: 0,
        bytes_per_row: Some(4 * w),
        rows_per_image: Some(h),
    };
    // In-place update when dimensions match.
    if let Some(it) = slot.as_ref().filter(|it| it.width == w && it.height == h) {
        rs.queue
            .write_texture(image_copy(&it.texture), pixels, layout, extent);
        return;
    }

    // Unregister the old texture if dimensions changed.
    if let Some(it) = slot.take() {
        rs.renderer.write().free_texture(&it.texture_id);
    }

    let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("patina_image"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    rs.queue
        .write_texture(image_copy(&texture), pixels, layout, extent);

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let texture_id =
        rs.renderer
            .write()
            .register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);

    *slot = Some(ImageTexture {
        texture,
        texture_id,
        width: w,
        height: h,
    });
}

// ── Save As modal ─────────────────────────────────────────────────────────────

fn draw_save_as_modal(ctx: &egui::Context, app: &mut App) {
    let Some(ref mut modal) = app.save_as_modal else {
        return;
    };
    use ui::tok::{BORDER, DANGER, SURFACE, TEXT_1, TEXT_2};

    // Dim at Background so the Window (Middle) is always above it.
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("saveas_dim"),
    ))
    .rect_filled(ctx.screen_rect(), 0.0, ui::tok::SCRIM);

    let mut confirm = false;
    let mut cancel = false;
    let mut browse = false;

    egui::Window::new("save_as_modal")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::none()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(24.0),
        )
        .show(ctx, |ui| {
            ui.set_width(360.0);
            // Reset global style overrides so modal widgets render at full opacity.
            ui.visuals_mut().override_text_color = None;
            ui.visuals_mut().widgets.noninteractive.fg_stroke.color = TEXT_1;
            ui.visuals_mut().widgets.inactive.fg_stroke.color = TEXT_1;

            ui.label(
                egui::RichText::new("Save As")
                    .color(TEXT_1)
                    .font(ui::title_font(ui::TITLE)),
            );
            ui.add_space(16.0);

            // Folder row
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Folder").color(TEXT_2).size(12.0));
                ui.add_space(4.0);
                let dir_str = modal.directory.to_string_lossy();
                let display = truncate_path_display(&dir_str, 38);
                ui.label(egui::RichText::new(display).color(TEXT_1).size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::Button::new(ui::Kind::Secondary, "Browse…")
                        .icon(None)
                        .show(ui)
                        .clicked()
                    {
                        browse = true;
                    }
                });
            });
            ui.add_space(12.0);

            // Filename field
            ui.label(egui::RichText::new("Filename").color(TEXT_2).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut modal.stem)
                        .desired_width(260.0)
                        .font(egui::FontId::proportional(13.0)),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                ui.label(
                    egui::RichText::new(format!(".{}", modal.format.extension()))
                        .color(TEXT_2)
                        .size(13.0),
                );
            });
            ui.add_space(12.0);

            // Export preset selector
            ui.label(egui::RichText::new("Preset").color(TEXT_2).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for &(preset, label) in ExportPreset::ALL {
                    if ui::Button::new(ui::Kind::Secondary, label)
                        .size(ui::BtnSize::CHIP)
                        .selected(modal.preset == preset)
                        .show(ui)
                        .clicked()
                    {
                        modal.preset = preset;
                        ExportPreset::apply(preset, modal);
                    }
                }
            });
            ui.add_space(12.0);

            // Format selector
            ui.label(egui::RichText::new("Format").color(TEXT_2).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for &fmt in OutputFormat::ALL {
                    if ui::Button::new(ui::Kind::Secondary, fmt.label())
                        .size(ui::BtnSize::CHIP)
                        .selected(modal.format == fmt)
                        .show(ui)
                        .clicked()
                    {
                        modal.format = fmt;
                        modal.preset = ExportPreset::Custom;
                    }
                }
            });
            ui.add_space(8.0);

            // Quality slider (JPEG / WebP only)
            if matches!(modal.format, OutputFormat::Jpeg | OutputFormat::WebP) {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Quality").color(TEXT_2).size(12.0));
                    ui.add_space(8.0);
                    let mut q = modal.quality as f32;
                    if ui
                        .add(
                            egui::Slider::new(&mut q, 1.0..=100.0)
                                .show_value(true)
                                .clamp_to_range(true),
                        )
                        .changed()
                    {
                        modal.quality = q as u8;
                        modal.preset = ExportPreset::Custom;
                    }
                });
                ui.add_space(8.0);
            }

            // Max dimension (optional resize)
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Long edge").color(TEXT_2).size(12.0));
                ui.add_space(8.0);
                let mut enabled = modal.max_dim.is_some();
                if ui.checkbox(&mut enabled, "").changed() {
                    modal.max_dim = if enabled { Some(2048) } else { None };
                    modal.preset = ExportPreset::Custom;
                }
                if let Some(ref mut dim) = modal.max_dim {
                    let mut d = *dim as f32;
                    if ui
                        .add(
                            egui::Slider::new(&mut d, 256.0..=8192.0)
                                .step_by(64.0)
                                .show_value(true)
                                .suffix("px"),
                        )
                        .changed()
                    {
                        *dim = d as u32;
                        modal.preset = ExportPreset::Custom;
                    }
                }
            });
            ui.add_space(16.0);

            ui.label(
                egui::RichText::new(format!("→ {}", modal.full_path().display()))
                    .color(TEXT_2)
                    .size(11.0),
            );
            let replacing = modal.replace.as_ref() == Some(&modal.full_path());
            if replacing {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} already exists. Replace it?",
                        modal.full_filename()
                    ))
                    .color(DANGER)
                    .size(12.0),
                );
            }
            ui.add_space(16.0);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (label, kind) = if replacing {
                    ("Replace", ui::Kind::Danger)
                } else {
                    ("Save", ui::Kind::Primary)
                };
                if ui::Button::new(kind, label).icon(None).show(ui).clicked() {
                    confirm = true;
                }
                if ui::Button::new(ui::Kind::Secondary, "Cancel")
                    .icon(None)
                    .show(ui)
                    .clicked()
                {
                    cancel = true;
                }
            });
        });

    if browse {
        let start = app.save_as_modal.as_ref().map(|m| m.directory.clone());
        if let Some(dir) = {
            let mut d = rfd::FileDialog::new().set_title("Choose folder");
            if let Some(s) = start {
                d = d.set_directory(s);
            }
            d.pick_folder()
        } {
            if let Some(m) = &mut app.save_as_modal {
                m.directory = dir;
            }
        }
    }
    // Esc closes Save As; the editor's own Esc stands down while it is open.
    if ctx.input_mut(|i| consume_shortcut(i, egui::Modifiers::NONE, egui::Key::Escape)) {
        cancel = true;
    }
    if cancel {
        app.save_as_modal = None;
    }
    if confirm {
        confirm_save_as(app);
    }
}

/// Save As's Save button or Enter. A file already at the target is replaced
/// only on a second confirm of the same path, after the dialog has said so;
/// before, it was replaced without a word.
fn confirm_save_as(app: &mut App) {
    let Some(m) = &mut app.save_as_modal else {
        return;
    };
    let dest = m.full_path();
    if dest.exists() && m.replace.as_ref() != Some(&dest) {
        m.replace = Some(dest);
        return;
    }
    let (q, d) = (m.quality, m.max_dim);
    app.save_as_modal = None;
    app.commit_save_as(dest, q, d);
}

// ── Main modal ────────────────────────────────────────────────────────────────

/// Draw `modal` and return the button the user clicked, if any. It only reads;
/// the caller applies the action with `&mut App`, so nothing here borrows the
/// app while `icons` (which lives in it) is borrowed too.
fn draw_modal(
    ctx: &egui::Context,
    modal: &ModalDialog,
    icons: Option<&ui::Icons>,
) -> Option<ModalAction> {
    use ui::tok::{BORDER, DANGER, SURFACE, TEXT_1, TEXT_2};

    // Dim overlay at Order::Background so the Window (Order::Middle) renders above it.
    // Using Foreground here would paint over the Window, making it appear greyed out.
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("modal_dim"),
    ))
    .rect_filled(ctx.screen_rect(), 0.0, ui::tok::SCRIM);

    let mut action: Option<ModalAction> = None;

    egui::Window::new("modal")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::none()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(24.0),
        )
        .show(ctx, |ui| {
            ui.set_width(340.0);
            // Reset global style overrides so modal text renders at full opacity.
            // apply_panel_style sets override_text_color and fg_stroke globally,
            // which bleeds into Window widgets and makes them appear disabled.
            ui.visuals_mut().override_text_color = None;
            ui.visuals_mut().widgets.noninteractive.fg_stroke.color = TEXT_1;
            ui.visuals_mut().widgets.inactive.fg_stroke.color = TEXT_1;

            let btn_row = |ui: &mut egui::Ui, f: &mut dyn FnMut(&mut egui::Ui)| {
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    f(ui);
                });
            };
            match modal {
                ModalDialog::SaveOnExit { .. } => {
                    ui.label(
                        egui::RichText::new("Save changes?")
                            .color(TEXT_1)
                            .font(ui::title_font(ui::TITLE)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Your edits will be lost if you don't save.")
                            .color(TEXT_2)
                            .size(13.0),
                    );
                    btn_row(ui, &mut |ui| {
                        if ui::Button::new(ui::Kind::Primary, "Save")
                            .icon(icons.map(|ic| &ic.save))
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Save);
                        }
                        if ui::Button::new(ui::Kind::Secondary, "Discard")
                            .icon(None)
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Discard);
                        }
                        if ui::Button::new(ui::Kind::Secondary, "Cancel")
                            .icon(None)
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Dismiss);
                        }
                    });
                }
                ModalDialog::ConfirmDelete { path } => {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    ui.label(
                        egui::RichText::new("Delete image?")
                            .color(TEXT_1)
                            .font(ui::title_font(ui::TITLE)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("Move\n\n{filename}\n\nto the Recycle Bin?"))
                            .color(TEXT_2)
                            .size(13.0),
                    );
                    btn_row(ui, &mut |ui| {
                        if ui::Button::new(ui::Kind::Danger, "Delete")
                            .icon(icons.map(|ic| &ic.reset))
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Delete);
                        }
                        if ui::Button::new(ui::Kind::Secondary, "Cancel")
                            .icon(None)
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Dismiss);
                        }
                    });
                }
                ModalDialog::OfferRename {
                    old_ext,
                    new_ext,
                    new_path,
                    ..
                } => {
                    ui.label(
                        egui::RichText::new("Wrong file extension")
                            .color(TEXT_1)
                            .font(ui::title_font(ui::TITLE)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "This file is a .{new_ext} saved as .{old_ext}.\n\nRename to: {}",
                            new_path.file_name().unwrap_or_default().to_string_lossy()
                        ))
                        .color(TEXT_2)
                        .size(13.0),
                    );
                    btn_row(ui, &mut |ui| {
                        if ui::Button::new(ui::Kind::Primary, "Rename")
                            .icon(icons.map(|ic| &ic.save))
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Rename);
                        }
                        if ui::Button::new(ui::Kind::Secondary, "Keep")
                            .icon(None)
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Dismiss);
                        }
                    });
                }
                ModalDialog::Error { message } => {
                    ui.label(
                        egui::RichText::new("Error")
                            .color(DANGER)
                            .font(ui::title_font(ui::TITLE)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(message.as_str())
                            .color(TEXT_1)
                            .size(13.0),
                    );
                    btn_row(ui, &mut |ui| {
                        if ui::Button::new(ui::Kind::Secondary, "OK")
                            .icon(None)
                            .show(ui)
                            .clicked()
                        {
                            action = Some(ModalAction::Dismiss);
                        }
                    });
                }
            }
        });

    // The keyboard is the dialog's while it is open (process_keyboard stands
    // down): Esc is always the way out that changes nothing, and Enter takes
    // the first button. The delete goes to the Recycle Bin, so Enter may
    // delete, as in Explorer.
    let (enter, esc) = ctx.input_mut(|i| {
        (
            consume_shortcut(i, egui::Modifiers::NONE, egui::Key::Enter),
            consume_shortcut(i, egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    action.or(if esc {
        Some(ModalAction::Dismiss)
    } else if enter {
        Some(match modal {
            ModalDialog::SaveOnExit { .. } => ModalAction::Save,
            ModalDialog::ConfirmDelete { .. } => ModalAction::Delete,
            ModalDialog::OfferRename { .. } => ModalAction::Rename,
            ModalDialog::Error { .. } => ModalAction::Dismiss,
        })
    } else {
        None
    })
}

/// Type-safe modal action — replaces the stringly-typed `&str` approach.
/// `Dismiss` is Cancel, Keep or OK: close without doing anything.
enum ModalAction {
    Save,
    Discard,
    Delete,
    Rename,
    Dismiss,
}

/// Apply a clicked modal button. Reads `app.modal`, so the caller clears it
/// afterwards.
fn apply_modal_action(app: &mut App, action: ModalAction) {
    match action {
        ModalAction::Dismiss => {}
        ModalAction::Save => {
            if let Some(ModalDialog::SaveOnExit { save_and_quit }) = &app.modal {
                app.pending_save_and_exit = Some(*save_and_quit);
            }
        }
        ModalAction::Discard => {
            // Discarding on the way out still goes out; before, Q and Discard
            // only left the editor.
            if let Some(ModalDialog::SaveOnExit {
                save_and_quit: true,
            }) = &app.modal
            {
                app.pending_close = true;
            }
            app.do_exit_editor();
        }
        ModalAction::Delete => {
            if let Some(ModalDialog::ConfirmDelete { path }) = &app.modal {
                app.pending_delete = Some(path.clone());
            }
        }
        ModalAction::Rename => {
            if let Some(ModalDialog::OfferRename {
                old_path, new_path, ..
            }) = &app.modal
            {
                let (old, new) = (old_path.clone(), new_path.clone());
                match rename_no_replace(&old, &new) {
                    Ok(()) => {
                        app.navigator.rename(&old, new.clone());
                        if let Some(name) = new.file_name() {
                            app.toast
                                .show(format!("Renamed to {}", name.to_string_lossy()));
                        }
                        app.current_path = Some(new);
                    }
                    Err(e) => {
                        app.pending_error =
                            Some(format!("Could not rename {}:\n{e}", old.display()))
                    }
                }
            }
        }
    }
}

// ── PanelAction extension ─────────────────────────────────────────────────────
//
// Methods on PanelAction used in App::apply_panel_action and draw_editor_panel.
// PanelAction is defined in ui.rs — these extension methods avoid having to
// repeat the same field checks in multiple places.

trait PanelActionExt {
    /// True if any field that would trigger an explicit (non-slider) action is set.
    fn has_explicit_edit(&self) -> bool;
    /// True if any field at all is set (used to skip no-op frames).
    fn has_any(&self) -> bool;
}

impl PanelActionExt for PanelAction {
    fn has_explicit_edit(&self) -> bool {
        self.save
            || self.save_as
            || self.undo
            || self.redo
            || self.reset_all
            || self.rot_cw
            || self.rot_ccw
            || self.flip_h
            || self.flip_v
    }
    fn has_any(&self) -> bool {
        self.has_explicit_edit() || self.new_params.is_some() || self.drag_params.is_some()
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Move `path` to the Recycle Bin (or the platform's trash), so a delete can
/// be undone. Where a volume has no Recycle Bin, Windows asks before
/// destroying the file.
///
/// `trash` unwraps a failed COM instantiation internally. Patina does not
/// abort on panic, so an uncaught one would end only the delete thread: no
/// `Msg::Deleted`, and `deleting` stuck at true would block every later
/// delete. Caught here, it becomes an error the user sees.
fn move_to_trash(path: &Path) -> Result<(), String> {
    std::panic::catch_unwind(|| trash::delete(path))
        .map_err(|_| "the Recycle Bin could not be reached".to_string())?
        .map_err(|e| e.to_string())
}

/// Clamp `w×h` to 90% of the screen rect, with a minimum of `MIN_WINDOW`.
fn clamp_to_work_area(w: u32, h: u32, ctx: &egui::Context) -> (u32, u32) {
    let screen = ctx.screen_rect();
    let max_w = (screen.width() * 0.90) as u32;
    let max_h = (screen.height() * 0.90) as u32;
    (
        w.min(max_w).max(MIN_WINDOW.0 as u32),
        h.min(max_h).max(MIN_WINDOW.1 as u32),
    )
}

/// Truncate a path display string to `max_chars`, prefixing with `…`.
fn truncate_path_display(s: &str, max_chars: usize) -> String {
    if s.len() > max_chars {
        format!("…{}", &s[s.len() - max_chars..])
    } else {
        s.to_string()
    }
}

fn offer_rename_if_writable(path: &Path, correct_ext: &str) -> Option<PathBuf> {
    // Probe writability by opening for write (no truncate). Best cross-platform approach.
    std::fs::OpenOptions::new().write(true).open(path).ok()?;
    Some(unique_rename_target(path, correct_ext))
}

/// Rename `from` to `to` unless `to` exists. `std::fs::rename` replaces an
/// existing `to` on Windows, and the offered name, unused when the dialog
/// opened, may have been taken by the time Rename is clicked. The check and
/// the rename are still two calls; std's only atomic no-replace is a hard
/// link, which exFAT memory cards do not have.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.try_exists()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    std::fs::rename(from, to)
}

fn unique_rename_target(path: &Path, new_ext: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    let base = dir.join(format!("{}.{}", stem.to_string_lossy(), new_ext));
    if !base.exists() {
        return base;
    }
    for i in 2u32.. {
        let candidate = dir.join(format!("{}_{}.{}", stem.to_string_lossy(), i, new_ext));
        if !candidate.exists() {
            return candidate;
        }
    }
    base // unreachable in practice
}

/// Floating-point equality test used for change detection in `EditParams`.
/// Discrete fields (rotation, flip, preset) use `==`; continuous fields use ε.
fn params_equal(a: &EditParams, b: &EditParams) -> bool {
    a.rotation == b.rotation
        && a.flip_h == b.flip_h
        && a.flip_v == b.flip_v
        && a.preset == b.preset
        && close(a.exposure, b.exposure)
        && close(a.highlights, b.highlights)
        && close(a.shadows, b.shadows)
        && close(a.brightness, b.brightness)
        && close(a.contrast, b.contrast)
        && close(a.saturation, b.saturation)
        && close(a.vibrance, b.vibrance)
        && close(a.sharpen, b.sharpen)
}

#[inline]
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// ── Lossless JPEG geometry transforms ─────────────────────────────────────────

/// Returns `true` when params contain only rotation/flip — no tonal, colour,
/// detail or preset changes. These can be applied losslessly to JPEG files.
fn is_pure_geometry(p: &EditParams) -> bool {
    p.preset == editor::Preset::None
        && p.exposure.abs() < 1e-4
        && p.highlights.abs() < 1e-4
        && p.shadows.abs() < 1e-4
        && p.brightness.abs() < 1e-4
        && p.contrast.abs() < 1e-4
        && p.saturation.abs() < 1e-4
        && p.vibrance.abs() < 1e-4
        && p.sharpen.abs() < 1e-4
}

fn is_jpeg_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg") | Some("jpeg")
    )
}

fn load_icon() -> egui::IconData {
    // To embed the window icon, place a 256×256 RGBA PNG at src/assets/icon.png
    // and replace this with:
    //   let bytes = include_bytes!("assets/icon.png");
    //   let img   = image::load_from_memory(bytes).unwrap().into_rgba8();
    //   let (w, h) = img.dimensions();
    //   egui::IconData { rgba: img.into_raw(), width: w, height: h }
    egui::IconData::default()
}

fn image_size_hint(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    logging::init();

    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);
    let (init_w, init_h) = initial_path
        .as_deref()
        .and_then(image_size_hint)
        .unwrap_or(DEFAULT_WINDOW);

    let (msg_tx, msg_rx) = bounded::<Msg>(32);
    let app = App::new(msg_tx, msg_rx);

    // Pre-queue the initial image load before eframe starts.
    // The message will be waiting in msg_rx on the first update() call.
    if let Some(path) = initial_path {
        let tx = app.msg_tx.clone();
        std::thread::spawn(move || {
            let msg = match load_image(&path) {
                LoadOutcome::Ok(img) => Msg::Loaded {
                    path,
                    image: img,
                    mode: OpenMode::FirstImage,
                },
                LoadOutcome::WrongExtension(img, ext) => Msg::WrongExtension {
                    path,
                    image: img,
                    mode: OpenMode::FirstImage,
                    correct_ext: ext,
                },
                LoadOutcome::Unsupported(reason) => Msg::Unsupported {
                    path,
                    reason,
                    mode: OpenMode::FirstImage,
                },
            };
            let _ = tx.send(msg);
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size(egui::vec2(init_w as f32, init_h as f32))
            .with_min_inner_size(egui::vec2(MIN_WINDOW.0, MIN_WINDOW.1))
            .with_app_id(APP_NAME)
            .with_icon(load_icon()),
        // Patina's palette is dark only; following a light OS theme gave
        // egui's own widgets (text fields, checkboxes) light defaults inside
        // dark dialogs.
        follow_system_theme: false,
        default_theme: eframe::Theme::Dark,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode: eframe::wgpu::PresentMode::AutoVsync,
            power_preference: eframe::wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            ui::install_fonts(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Box::new(app) as Box<dyn eframe::App>
        }),
    )
    .expect("eframe failed to start");
}

#[cfg(test)]
mod trash_tests {
    use super::*;

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let path =
            std::env::temp_dir().join(format!("patina-no-such-file-{}.jpg", std::process::id()));
        assert!(move_to_trash(&path).is_err());
    }

    /// Puts a real file in the real Recycle Bin, so it is not run by default:
    /// `cargo test -- --ignored recycle`. Then check the bin for the file.
    #[test]
    #[ignore = "writes to the user's Recycle Bin"]
    fn a_file_goes_to_the_recycle_bin() {
        let path =
            std::env::temp_dir().join(format!("patina-recycle-test-{}.txt", std::process::id()));
        std::fs::write(&path, "patina recycle test").unwrap();

        move_to_trash(&path).unwrap();

        assert!(!path.exists());
    }
}

/// Keyboard and dialog paths that write or delete the user's files, driven
/// the way eframe drives the app: real key events through an egui context,
/// the same per-frame order as `update` minus the GPU upload, and the files
/// checked on disk afterwards.
#[cfg(test)]
mod keyboard_tests {
    use super::*;
    use egui::{Key, Modifiers};

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("patina-keys-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn jpeg(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            image::RgbImage::from_pixel(16, 8, image::Rgb([200, 60, 40]))
                .save(&path)
                .unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The app showing `path` in the viewer, as after it loaded.
    fn app_on(path: &Path) -> (App, egui::Context) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new(tx, rx);
        let (LoadOutcome::Ok(img) | LoadOutcome::WrongExtension(img, _)) = load_image(path) else {
            panic!("{} did not load", path.display())
        };
        app.navigator.set_current(path.to_path_buf());
        app.current_path = Some(path.to_path_buf());
        app.current_image = Some(img);
        let ctx = egui::Context::default();
        ui::install_fonts(&ctx); // as the app does at start-up
        (app, ctx)
    }

    /// Open the editor and make an unsaved edit.
    fn edit(app: &mut App, ctx: &egui::Context) {
        app.enter_editor(ctx);
        let p = EditParams {
            exposure: 0.5,
            ..EditParams::default()
        };
        app.editor_state.as_mut().unwrap().commit(p);
        assert!(app.editor_params_changed());
    }

    fn press(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// One frame with `events`, and with the window's close button pressed
    /// when `close` is set; returns the window commands the app sent.
    fn run(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        close: bool,
    ) -> Vec<egui::ViewportCommand> {
        let mut raw = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        if close {
            raw.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    events: vec![egui::ViewportEvent::Close],
                    ..Default::default()
                },
            );
        }
        let out = ctx.run(raw, |ctx| {
            app.apply_pending_actions(ctx);
            app.guard_close(ctx);
            app.draw_ui(ctx);
            app.process_keyboard(ctx);
            app.sync_title(ctx);
        });
        out.viewport_output
            .into_values()
            .flat_map(|v| v.commands)
            .collect()
    }

    /// One frame with `events`; returns whether it asked the window to close.
    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> bool {
        run(app, ctx, events, false).contains(&egui::ViewportCommand::Close)
    }

    const CTRL: Modifiers = Modifiers::COMMAND;
    const CTRL_SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::COMMAND
    };
    // What winit reports on Windows for Ctrl: both flags set.
    const WIN_CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::COMMAND
    };
    const WIN_CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        ..Modifiers::COMMAND
    };

    #[test]
    fn ctrl_shift_s_opens_save_as_and_leaves_the_file_alone() {
        let dir = TempDir::new("saveas");
        let path = dir.jpeg("photo.jpg");
        let before = std::fs::read(&path).unwrap();
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx);

        frame(&mut app, &ctx, vec![press(Key::S, WIN_CTRL_SHIFT)]);

        assert!(app.save_as_modal.is_some(), "Save As opened");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the original is untouched"
        );
        assert!(app.editor_params_changed(), "the edit is still unsaved");

        // Ctrl+S on its own still saves in place.
        frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.save_as_modal.is_none(), "Esc closed Save As");
        assert!(
            app.in_editor(),
            "the same Esc did not also leave the editor"
        );
        frame(&mut app, &ctx, vec![press(Key::S, WIN_CTRL)]);
        assert_ne!(
            std::fs::read(&path).unwrap(),
            before,
            "Ctrl+S wrote the file"
        );
    }

    #[test]
    fn ctrl_shift_z_redoes() {
        let dir = TempDir::new("redo");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        edit(&mut app, &ctx);

        frame(&mut app, &ctx, vec![press(Key::Z, CTRL)]);
        assert!(!app.editor_params_changed(), "Ctrl+Z undid");
        frame(&mut app, &ctx, vec![press(Key::Z, CTRL_SHIFT)]);
        assert!(
            app.editor_params_changed(),
            "Ctrl+Shift+Z redid instead of undoing again"
        );
    }

    #[test]
    fn esc_in_save_changes_keeps_the_edits() {
        let dir = TempDir::new("esc");
        let path = dir.jpeg("photo.jpg");
        let before = std::fs::read(&path).unwrap();
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx);

        frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(
            matches!(app.modal, Some(ModalDialog::SaveOnExit { .. })),
            "{:?}",
            app.modal
        );

        frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);

        assert!(app.modal.is_none(), "Esc dismissed the dialog");
        assert!(
            app.in_editor() && app.editor_params_changed(),
            "and kept the editor and the edit"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn typing_a_filename_in_save_as_runs_no_shortcuts() {
        let dir = TempDir::new("typing");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        edit(&mut app, &ctx);
        app.save_as_returning(&ctx);

        // Q quit, E left the editor, F went fullscreen, H opened help.
        let typed = [Key::Q, Key::E, Key::F, Key::H].map(|k| press(k, Modifiers::NONE));
        let closed = frame(&mut app, &ctx, typed.to_vec());

        assert!(!closed);
        assert!(app.modal.is_none(), "{:?}", app.modal);
        assert!(app.in_editor() && app.editor_params_changed());
        assert!(!app.show_help && !app.pending_fullscreen_toggle);
        assert!(app.save_as_modal.is_some());
    }

    #[test]
    fn the_delete_dialog_deletes_the_file_it_names() {
        let dir = TempDir::new("delete");
        let named = dir.jpeg("a.jpg");
        let other = dir.jpeg("b.jpg");
        let (mut app, ctx) = app_on(&named);

        frame(&mut app, &ctx, vec![press(Key::Delete, Modifiers::NONE)]);
        assert!(matches!(&app.modal, Some(ModalDialog::ConfirmDelete { path }) if *path == named));

        // The arrows no longer move the view under the dialog...
        frame(
            &mut app,
            &ctx,
            vec![press(Key::ArrowRight, Modifiers::NONE)],
        );
        assert_eq!(app.current_path.as_ref(), Some(&named));
        // ...and should anything else move it, the dialog still means a.jpg.
        app.current_path = Some(other.clone());
        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);

        assert_eq!(app.pending_delete.as_ref(), Some(&named));
        // Stop here: the next frame would send a.jpg to the real Recycle Bin.
        // `deletes_the_named_file_on_disk` below runs that step.
        app.pending_delete = None;
        assert!(named.exists() && other.exists());
    }

    /// Runs the whole delete, so it writes to the real Recycle Bin:
    /// `cargo test -- --ignored named_file`.
    #[test]
    #[ignore = "writes to the user's Recycle Bin"]
    fn deletes_the_named_file_on_disk() {
        let dir = TempDir::new("delete-disk");
        let named = dir.jpeg("patina-delete-test-a.jpg");
        let other = dir.jpeg("patina-delete-test-b.jpg");
        let (mut app, ctx) = app_on(&named);

        frame(&mut app, &ctx, vec![press(Key::Delete, Modifiers::NONE)]);
        app.current_path = Some(other.clone());
        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![]);
        let Ok(Msg::Deleted { path, result }) = app.msg_rx.recv_timeout(Duration::from_secs(10))
        else {
            panic!("no delete result")
        };

        assert_eq!(path, named);
        assert_eq!(result, Ok(()));
        assert!(!named.exists(), "the named file went to the Recycle Bin");
        assert!(other.exists(), "the one on screen did not");
        assert_eq!(
            app.current_path.as_ref(),
            Some(&other),
            "and stayed on screen"
        );
    }

    #[test]
    fn save_and_quit_with_a_failed_save_keeps_the_window_and_the_edits() {
        let dir = TempDir::new("quitfail");
        let path = dir.jpeg("photo.jpg");
        let before = std::fs::read(&path).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms.clone()).unwrap();
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx);

        frame(&mut app, &ctx, vec![press(Key::Q, Modifiers::NONE)]);
        assert!(
            matches!(
                app.modal,
                Some(ModalDialog::SaveOnExit {
                    save_and_quit: true
                })
            ),
            "{:?}",
            app.modal
        );
        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
        let closed = frame(&mut app, &ctx, vec![]);

        assert!(!closed, "the window stayed open");
        assert!(
            app.in_editor() && app.editor_params_changed(),
            "with the edits"
        );
        assert!(
            matches!(app.modal, Some(ModalDialog::Error { .. })),
            "and the error shown: {:?}",
            app.modal
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        #[allow(clippy::permissions_set_readonly_false)] // a test file, removed with its directory
        perms.set_readonly(false);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    #[test]
    fn save_and_quit_with_a_good_save_writes_then_closes() {
        let dir = TempDir::new("quitok");
        let path = dir.jpeg("photo.jpg");
        let before = std::fs::read(&path).unwrap();
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx);

        frame(&mut app, &ctx, vec![press(Key::Q, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
        let closed = frame(&mut app, &ctx, vec![]);

        assert!(closed);
        assert_ne!(std::fs::read(&path).unwrap(), before);
    }

    /// The saved pixels carry the committed edit even when no render has
    /// caught up with it (the slider debounce), and not a hovered preset.
    #[test]
    fn a_save_writes_the_committed_edit_not_the_last_frame() {
        let dir = TempDir::new("savefresh");
        let path = dir.jpeg("photo.jpg");
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx); // exposure +0.5, committed, never rendered
                              // The screen shows a hovered B&W preview meanwhile.
        let es = app.editor_state.as_mut().unwrap();
        let grey = EditParams {
            preset: editor::Preset::Bw,
            ..EditParams::default()
        };
        es.display = editor::apply_edits(app.current_image.as_ref().unwrap(), &grey);

        assert!(app.save_inplace());

        let saved = image::open(&path).unwrap().to_rgb8();
        let px = saved.get_pixel(8, 4);
        // Source is (200, 60, 40): brighter, and still red, not grey.
        assert!(px[1] > 70 && px[2] > 48, "exposure applied: {px:?}");
        assert!(
            px[0] as i32 - px[1] as i32 > 80,
            "not the B&W preview: {px:?}"
        );
    }

    /// The frames of one drag replace the step its first frame made, so one
    /// Ctrl+Z takes the whole drag back.
    #[test]
    fn one_undo_takes_back_a_whole_drag() {
        let dir = TempDir::new("dragundo");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        app.enter_editor(&ctx);
        let at = |exposure| EditParams {
            exposure,
            ..EditParams::default()
        };
        app.apply_panel_action(
            PanelAction {
                new_params: Some(at(0.1)),
                ..Default::default()
            },
            &ctx,
        );
        for e in [0.2, 0.4, 0.8] {
            app.apply_panel_action(
                PanelAction {
                    drag_params: Some(at(e)),
                    ..Default::default()
                },
                &ctx,
            );
        }
        let es = app.editor_state.as_ref().unwrap();
        assert!(close(es.undo.current().exposure, 0.8));

        frame(&mut app, &ctx, vec![press(Key::Z, WIN_CTRL)]);
        assert!(!app.editor_params_changed(), "one undo, back to the start");
    }

    #[test]
    fn i_toggles_the_info_card_in_both_modes() {
        let dir = TempDir::new("info");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        frame(&mut app, &ctx, vec![press(Key::I, Modifiers::NONE)]);
        assert!(app.show_info);
        app.enter_editor(&ctx);
        frame(&mut app, &ctx, vec![press(Key::I, Modifiers::NONE)]);
        assert!(!app.show_info);
    }

    // ── Closing the window ────────────────────────────────────────────────────

    #[test]
    fn closing_the_window_with_unsaved_edits_asks_first() {
        let dir = TempDir::new("close");
        let path = dir.jpeg("photo.jpg");
        let before = std::fs::read(&path).unwrap();
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx);

        let cmds = run(&mut app, &ctx, vec![], true);

        assert!(
            cmds.contains(&egui::ViewportCommand::CancelClose),
            "{cmds:?}"
        );
        assert!(
            matches!(
                app.modal,
                Some(ModalDialog::SaveOnExit {
                    save_and_quit: true
                })
            ),
            "{:?}",
            app.modal
        );
        // Esc: stay, with the edits.
        assert!(!frame(
            &mut app,
            &ctx,
            vec![press(Key::Escape, Modifiers::NONE)]
        ));
        assert!(app.in_editor() && app.editor_params_changed());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn discard_on_close_closes_without_writing() {
        let dir = TempDir::new("discard");
        let path = dir.jpeg("photo.jpg");
        let before = std::fs::read(&path).unwrap();
        let (mut app, ctx) = app_on(&path);
        edit(&mut app, &ctx);
        run(&mut app, &ctx, vec![], true);

        apply_modal_action(&mut app, ModalAction::Discard); // the Discard button
        app.modal = None;
        assert!(frame(&mut app, &ctx, vec![]), "Discard went on to close");
        // The close that follows is no longer stopped.
        assert!(!run(&mut app, &ctx, vec![], true).contains(&egui::ViewportCommand::CancelClose));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn closing_with_nothing_unsaved_just_closes() {
        let dir = TempDir::new("closeclean");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        app.enter_editor(&ctx);

        let cmds = run(&mut app, &ctx, vec![], true);

        assert!(
            !cmds.contains(&egui::ViewportCommand::CancelClose),
            "{cmds:?}"
        );
        assert!(app.modal.is_none());
    }

    // ── Save As onto an existing file ─────────────────────────────────────────

    #[test]
    fn save_as_asks_before_replacing_a_file() {
        let dir = TempDir::new("replace");
        let open = dir.jpeg("a.jpg");
        let other = dir.0.join("b.jpg");
        std::fs::write(&other, b"someone else's photo").unwrap();
        let (mut app, ctx) = app_on(&open);
        edit(&mut app, &ctx);
        app.save_as_returning(&ctx);
        app.save_as_modal.as_mut().unwrap().stem = "b".into();

        confirm_save_as(&mut app);

        assert_eq!(
            std::fs::read(&other).unwrap(),
            b"someone else's photo",
            "not replaced on the first Save"
        );
        assert_eq!(
            app.save_as_modal.as_ref().and_then(|m| m.replace.clone()),
            Some(other.clone())
        );
        frame(&mut app, &ctx, vec![]); // draws the warning and the Replace button

        // A different name is a new target: saved at once, b.jpg untouched.
        app.save_as_modal.as_mut().unwrap().stem = "c".into();
        confirm_save_as(&mut app);
        assert!(app.save_as_modal.is_none());
        assert!(dir.0.join("c.jpg").exists());
        assert_eq!(std::fs::read(&other).unwrap(), b"someone else's photo");
    }

    #[test]
    fn save_as_replaces_after_the_second_confirm() {
        let dir = TempDir::new("replace2");
        let open = dir.jpeg("a.jpg");
        let other = dir.0.join("b.jpg");
        std::fs::write(&other, b"someone else's photo").unwrap();
        let (mut app, ctx) = app_on(&open);
        edit(&mut app, &ctx);
        app.save_as_returning(&ctx);
        app.save_as_modal.as_mut().unwrap().stem = "b".into();

        confirm_save_as(&mut app);
        confirm_save_as(&mut app);

        assert!(app.save_as_modal.is_none());
        assert!(
            matches!(load_image(&other), LoadOutcome::Ok(_)),
            "b.jpg is now the edited image"
        );
    }

    // ── Wrong-extension rename ────────────────────────────────────────────────

    /// The offered name was free when the dialog opened, then a file took it.
    #[test]
    fn rename_never_replaces_a_file_that_appeared() {
        let dir = TempDir::new("rename");
        let old = dir.0.join("photo.jpg");
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .save_with_format(&old, image::ImageFormat::Png)
            .unwrap();
        let new = dir.0.join("photo.png");
        let (mut app, ctx) = app_on(&old);
        app.modal = Some(ModalDialog::OfferRename {
            old_path: old.clone(),
            new_path: new.clone(),
            old_ext: "jpg".into(),
            new_ext: "png".into(),
        });
        std::fs::write(&new, b"arrived since").unwrap();

        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);

        assert_eq!(std::fs::read(&new).unwrap(), b"arrived since");
        assert!(old.exists(), "the original keeps its name");
        assert_eq!(app.current_path.as_ref(), Some(&old));
        frame(&mut app, &ctx, vec![]);
        assert!(
            matches!(&app.modal, Some(ModalDialog::Error { message }) if message.contains("already exists")),
            "{:?}",
            app.modal
        );
    }

    #[test]
    fn rename_to_a_free_name_renames() {
        let dir = TempDir::new("rename-ok");
        let old = dir.0.join("photo.jpg");
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .save_with_format(&old, image::ImageFormat::Png)
            .unwrap();
        let new = dir.0.join("photo.png");
        let (mut app, ctx) = app_on(&old);
        app.modal = Some(ModalDialog::OfferRename {
            old_path: old.clone(),
            new_path: new.clone(),
            old_ext: "jpg".into(),
            new_ext: "png".into(),
        });

        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);

        assert!(!old.exists() && new.exists());
        assert_eq!(app.current_path.as_ref(), Some(&new));
    }

    // ── Title, keymap and guides ──────────────────────────────────────────────

    fn title(cmds: Vec<egui::ViewportCommand>) -> Option<String> {
        cmds.into_iter().find_map(|c| match c {
            egui::ViewportCommand::Title(t) => Some(t),
            _ => None,
        })
    }

    #[test]
    fn the_title_follows_navigation_and_unsaved_edits() {
        let dir = TempDir::new("title");
        let a = dir.jpeg("a.jpg");
        let b = dir.jpeg("b.jpg");
        dir.jpeg("c.jpg");
        let (mut app, ctx) = app_on(&a);

        let first = run(&mut app, &ctx, vec![], false);
        assert_eq!(title(first).as_deref(), Some("a.jpg — 1 / 3 — Patina"));
        assert_eq!(
            title(run(&mut app, &ctx, vec![], false)),
            None,
            "sent when it changes, not every frame"
        );

        // → loads b.jpg on a background thread, as in the app.
        frame(
            &mut app,
            &ctx,
            vec![press(Key::ArrowRight, Modifiers::NONE)],
        );
        for _ in 0..500 {
            if app.current_path.as_ref() == Some(&b) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            let _ = ctx.run(Default::default(), |ctx| {
                app.process_background_messages(ctx)
            });
        }
        assert_eq!(app.current_path.as_ref(), Some(&b), "b.jpg loaded");
        let after = run(&mut app, &ctx, vec![], false);
        assert_eq!(title(after).as_deref(), Some("b.jpg — 2 / 3 — Patina"));

        edit(&mut app, &ctx);
        let dirty = run(&mut app, &ctx, vec![], false);
        assert_eq!(title(dirty).as_deref(), Some("● b.jpg — 2 / 3 — Patina"));
    }

    #[test]
    fn a_lone_file_has_no_position() {
        assert_eq!(
            window_title(Some("a.jpg"), Some((1, 1)), false),
            "a.jpg — Patina"
        );
        assert_eq!(window_title(None, None, false), "Patina");
    }

    // ── Item 6: HUD, toasts, bad files, the empty state ───────────────────────

    /// Run frames that drain background messages until `done`, as the app's
    /// update loop would; panics after 5 s.
    fn pump_until(app: &mut App, ctx: &egui::Context, done: impl Fn(&App) -> bool) {
        for _ in 0..500 {
            if done(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
            let _ = ctx.run(Default::default(), |ctx| {
                app.process_background_messages(ctx)
            });
        }
        panic!("timed out");
    }

    /// Every piece of text this frame paints.
    fn painted_text(app: &mut App, ctx: &egui::Context) -> Vec<String> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(raw, |ctx| app.draw_ui(ctx));
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    /// A file that will not decode shows as a card, not a modal, and the
    /// arrows carry on past it.
    #[test]
    fn a_bad_file_is_a_card_and_browsing_goes_on() {
        let dir = TempDir::new("badfile");
        let a = dir.jpeg("a.jpg");
        let b = dir.0.join("b.jpg");
        std::fs::write(&b, b"not a jpeg at all").unwrap();
        let c = dir.jpeg("c.jpg");
        let (mut app, ctx) = app_on(&a);

        frame(
            &mut app,
            &ctx,
            vec![press(Key::ArrowRight, Modifiers::NONE)],
        );
        pump_until(&mut app, &ctx, |app| app.bad_file.is_some());
        frame(&mut app, &ctx, vec![]); // where an error modal would have opened

        assert!(app.modal.is_none(), "no modal: {:?}", app.modal);
        assert_eq!(app.current_path.as_ref(), Some(&b));
        assert!(
            app.current_image.is_none(),
            "a's pixels are not left on screen"
        );
        assert!(app.loading_path.is_none(), "b can be tried again");
        let text = painted_text(&mut app, &ctx);
        assert!(
            text.iter().any(|t| t == "Can\u{2019}t open b.jpg"),
            "{text:?}"
        );

        frame(
            &mut app,
            &ctx,
            vec![press(Key::ArrowRight, Modifiers::NONE)],
        );
        pump_until(&mut app, &ctx, |app| app.current_image.is_some());
        assert_eq!(app.current_path.as_ref(), Some(&c));
        assert!(app.bad_file.is_none());
    }

    #[test]
    fn a_rating_says_what_it_did_and_is_on_disk() {
        let dir = TempDir::new("ratetoast");
        let path = dir.jpeg("a.jpg");
        let (mut app, ctx) = app_on(&path);

        frame(&mut app, &ctx, vec![press(Key::Num3, Modifiers::NONE)]);

        assert_eq!(
            app.toast.current.as_ref().map(|(t, _)| t.as_str()),
            Some("Rated \u{2605}\u{2605}\u{2605}")
        );
        assert_eq!(
            xmp::read_labels(&path).rating,
            3,
            "read back from the sidecar"
        );
    }

    #[test]
    fn a_finished_delete_says_so() {
        let dir = TempDir::new("deltoast");
        let path = dir.jpeg("a.jpg");
        let (mut app, ctx) = app_on(&path);
        app.msg_tx
            .send(Msg::Deleted {
                path,
                result: Ok(()),
            })
            .unwrap();
        pump_until(&mut app, &ctx, |app| app.toast.current.is_some());
        assert_eq!(app.toast.current.unwrap().0, "Moved to the Recycle Bin");
    }

    /// A JPEG that was only rotated says its pixels were left alone.
    #[test]
    fn a_lossless_rotation_save_says_how_it_saved() {
        let dir = TempDir::new("rottoast");
        let path = dir.jpeg("a.jpg");
        let (mut app, ctx) = app_on(&path);
        app.enter_editor(&ctx);
        let p = EditParams {
            rotation: EditParams::default().rotation.cw(),
            ..EditParams::default()
        };
        app.editor_state.as_mut().unwrap().commit(p);

        assert!(app.save_inplace());

        let said = app.toast.current.as_ref().unwrap().0.clone();
        assert!(said.contains("EXIF"), "{said}");
    }

    /// With nothing open the window says what to do, and E says why it
    /// does nothing.
    #[test]
    fn with_nothing_open_the_app_says_what_to_do() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new(tx, rx);
        let ctx = egui::Context::default();
        ui::install_fonts(&ctx);

        let text = painted_text(&mut app, &ctx);
        assert!(text.iter().any(|t| t == "Drop an image here"), "{text:?}");
        assert!(text.iter().any(|t| t == "or press Ctrl+O"), "{text:?}");

        frame(&mut app, &ctx, vec![press(Key::E, Modifiers::NONE)]);
        assert!(!app.in_editor());
        assert_eq!(
            app.toast.current.as_ref().map(|(t, _)| t.as_str()),
            Some("Open an image to edit it")
        );
    }

    #[test]
    fn g_steps_through_the_guides_and_tab_does_not() {
        let dir = TempDir::new("guides");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        app.enter_editor(&ctx);

        let mut seen = vec![app.guides];
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![press(Key::G, Modifiers::NONE)]);
            seen.push(app.guides);
        }
        assert_eq!(
            seen,
            [Guides::Off, Guides::Thirds, Guides::Golden, Guides::Off]
        );

        frame(&mut app, &ctx, vec![press(Key::Tab, Modifiers::NONE)]);
        assert_eq!(app.guides, Guides::Off);
    }

    /// Whether this frame paints a button-sized rect filled with the accent,
    /// which in the editor is only the primary button (a moved slider's 4 px
    /// fill is accent too, so size tells them apart).
    fn paints_accent_fill(app: &mut App, ctx: &egui::Context) -> bool {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(raw, |ctx| app.draw_ui(ctx));
        out.shapes.iter().any(|s| {
            matches!(&s.shape, egui::Shape::Rect(r)
                    if r.fill == ui::tok::ACCENT && r.rect.height() >= 20.0)
        })
    }

    #[test]
    fn save_is_the_primary_button_only_with_unsaved_edits() {
        let dir = TempDir::new("primary");
        let (mut app, ctx) = app_on(&dir.jpeg("photo.jpg"));
        app.enter_editor(&ctx);
        paints_accent_fill(&mut app, &ctx); // first frame lays out the panel

        assert!(
            !paints_accent_fill(&mut app, &ctx),
            "untouched: no green Save"
        );
        let p = EditParams {
            exposure: 0.5,
            ..EditParams::default()
        };
        app.editor_state.as_mut().unwrap().commit(p);
        assert!(
            paints_accent_fill(&mut app, &ctx),
            "edited: Save is primary"
        );
    }

    /// Tab reaches the dialog's buttons now that it is not a shortcut. Enter
    /// on a focused Cancel must cancel, not take the first button (Delete).
    #[test]
    fn enter_on_a_focused_cancel_cancels_the_delete() {
        let dir = TempDir::new("focus");
        let path = dir.jpeg("a.jpg");
        let (mut app, ctx) = app_on(&path);
        frame(&mut app, &ctx, vec![press(Key::Delete, Modifiers::NONE)]);
        assert!(matches!(app.modal, Some(ModalDialog::ConfirmDelete { .. })));

        frame(&mut app, &ctx, vec![press(Key::Tab, Modifiers::NONE)]); // Delete
        frame(&mut app, &ctx, vec![press(Key::Tab, Modifiers::NONE)]); // Cancel
        frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![]);

        assert!(app.modal.is_none(), "{:?}", app.modal);
        assert_eq!(app.pending_delete, None, "nothing was deleted");
        assert!(path.exists());
    }

    /// The rating and label keys go through the keymap now; what they write
    /// is the sidecar on disk.
    #[test]
    fn rating_and_label_keys_write_the_sidecar() {
        let dir = TempDir::new("rate");
        let path = dir.jpeg("photo.jpg");
        let (mut app, ctx) = app_on(&path);

        frame(&mut app, &ctx, vec![press(Key::Num3, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![press(Key::L, Modifiers::NONE)]);

        let on_disk = xmp::read_labels(&path);
        assert!(xmp::sidecar_path(&path).exists());
        assert_eq!(on_disk.rating, 3);
        assert_eq!(on_disk.colour, ColourLabel::Red);

        frame(&mut app, &ctx, vec![press(Key::Num0, Modifiers::NONE)]);
        assert_eq!(xmp::read_labels(&path).rating, 0);
    }
}
