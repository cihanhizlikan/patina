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

/// Timestamped debug logger — shared clock across all threads.
static LOG_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
macro_rules! log {
    ($($arg:tt)*) => {{
        let t = LOG_START.get_or_init(std::time::Instant::now).elapsed();
        eprintln!("[{:6}.{:03}] {}", t.as_secs(), t.subsec_millis(), format_args!($($arg)*));
    }};
}
fn init_log() { LOG_START.get_or_init(std::time::Instant::now); }

mod loader;
mod nav;
mod editor;
mod ui;
mod preset_thumbs;
mod exif;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::egui_wgpu::wgpu;
use loader::{load_image, DecodedImage, LoadOutcome};
use nav::DirNavigator;
use notify_debouncer_mini::{new_debouncer, Debouncer};
use editor::{EditorState, EditParams, apply_edits};
use ui::{draw_panel, ThumbCache, Icons, PanelAction};
use exif::ImageMetadata;

// ── Type aliases ──────────────────────────────────────────────────────────────

type DirWatcher = Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>;

// ── Constants ─────────────────────────────────────────────────────────────────

const APP_NAME:           &str     = "Patina";
const DIR_WATCH_DEBOUNCE: Duration = Duration::from_millis(50);
/// Slider debounce: wait this long after the last change before re-encoding.
const SLIDER_DEBOUNCE_MS: u128     = 80;
/// Minimum window dimensions on startup.
const MIN_WINDOW:         (f32, f32) = (400.0, 300.0);
/// Default window size when no image is provided at launch.
const DEFAULT_WINDOW:     (u32, u32) = (900, 680);
/// Canvas background colour — matches ui::CANVAS (#0e0e10).
const CANVAS_COLOR: egui::Color32 = egui::Color32::from_rgb(0x0E, 0x0E, 0x10);
/// How long each image is shown during slideshow.
const SLIDESHOW_INTERVAL: Duration = Duration::from_secs(5);

// ── Inter-thread messages ─────────────────────────────────────────────────────

#[derive(Debug)]
enum Msg {
    Loaded         { path: PathBuf, image: DecodedImage, mode: OpenMode },
    WrongExtension { path: PathBuf, image: DecodedImage, mode: OpenMode, correct_ext: String },
    Unsupported    { path: PathBuf, reason: String },
    Preloaded      { path: PathBuf, image: DecodedImage },
    Deleted        { path: PathBuf },
    MetadataLoaded { path: PathBuf, meta: ImageMetadata },
    DirectoryChanged,
}

#[derive(Default)]
struct DrainResult {
    resize_to:    Option<(u32, u32)>,
    error_msg:    Option<String>,
    rename_offer: Option<(PathBuf, String)>,
}

// ── Modal dialog variants ─────────────────────────────────────────────────────

#[derive(Debug)]
enum ModalDialog {
    SaveOnExit    { save_and_quit: bool },
    ConfirmDelete { filename: String },
    OfferRename   { old_path: PathBuf, new_path: PathBuf, old_ext: String, new_ext: String },
    Error         { message: String },
}

// ── GPU image texture ─────────────────────────────────────────────────────────

/// Wgpu texture registered with egui via `register_native_texture`.
/// The `texture_id` is a stable handle — egui never touches the pixel data.
struct ImageTexture {
    texture:    wgpu::Texture,
    texture_id: egui::TextureId,
    width:      u32,
    height:     u32,
}

// ── Save As modal state ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OutputFormat { Png, Jpeg, WebP, Tiff, Bmp }

impl OutputFormat {
    const ALL: &'static [Self] = &[Self::Png, Self::Jpeg, Self::WebP, Self::Tiff, Self::Bmp];

    fn label(self) -> &'static str {
        match self {
            Self::Png  => "PNG",  Self::Jpeg => "JPEG",
            Self::WebP => "WebP", Self::Tiff => "TIFF", Self::Bmp => "BMP",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png  => "png",  Self::Jpeg => "jpg",
            Self::WebP => "webp", Self::Tiff => "tif", Self::Bmp => "bmp",
        }
    }

    fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase()).as_deref()
        {
            Some("jpg") | Some("jpeg") => Self::Jpeg,
            Some("webp")               => Self::WebP,
            Some("tif") | Some("tiff") => Self::Tiff,
            Some("bmp")                => Self::Bmp,
            _                          => Self::Png,
        }
    }
}

struct SaveAsModal {
    stem:      String,   // filename without extension
    format:    OutputFormat,
    directory: PathBuf,
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
enum OpenMode { FirstImage, Navigate }

/// Which guide overlay to draw on the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum GuideType { #[default] RuleOfThirds, GoldenRatio }

struct App {
    // ── Image display ─────────────────────────────────────────────────────────
    current_path:         Option<PathBuf>,
    current_image:        Option<DecodedImage>,
    frame_index:          usize,
    next_frame_at:        Option<Instant>,
    image_texture:        Option<ImageTexture>,
    last_uploaded_gen:    u64,
    last_uploaded_frame:  usize,

    // ── Navigation & loading ──────────────────────────────────────────────────
    navigator:            DirNavigator,
    msg_tx:               Sender<Msg>,
    msg_rx:               Receiver<Msg>,
    loading_path:         Option<PathBuf>,
    preload_cache:        HashMap<PathBuf, DecodedImage>,
    deleting:             bool,
    dir_watcher:          Option<DirWatcher>,
    watched_dir:          Option<PathBuf>,

    // ── Window state ──────────────────────────────────────────────────────────
    fullscreen:               bool,
    pending_fullscreen_toggle: bool,
    maximized:                bool,
    pre_maximize_size:        Option<egui::Vec2>,
    image_gen:                u64,
    /// Resizes the window exactly once on the first image launch.
    has_shown_first_image:    bool,

    // ── Slideshow ─────────────────────────────────────────────────────────────
    slideshow_active:  bool,
    slideshow_next_at: Option<Instant>,

    // ── Canvas view state (editor mode only) ──────────────────────────────────
    /// Zoom multiplier: 1.0 = fit to canvas. Ctrl+scroll adjusts; Enter resets.
    zoom:          f32,
    /// Pan offset in logical pixels from the centred position.
    pan:           egui::Vec2,
    /// Whether the guide overlay is currently visible.
    show_guides:   bool,
    /// Which guide type is active when guides are shown.
    guide_type:    GuideType,

    // ── Metadata ──────────────────────────────────────────────────────────────
    /// Cached EXIF/IPTC metadata for the current image.
    metadata:      Option<ImageMetadata>,

    // ── Editor ────────────────────────────────────────────────────────────────
    editor_state:          Option<EditorState>,
    initial_editor_params: Option<EditParams>,
    editor_open_sections:  [bool; 6],
    thumb_cache:           Option<ThumbCache>,
    icon_cache:            Option<Icons>,

    // ── Async pixel editing ───────────────────────────────────────────────────
    edit_rx:              Option<crossbeam_channel::Receiver<(u64, DecodedImage)>>,
    edit_gen:             u64,
    last_slider_change:   Option<Instant>,

    // ── Deferred / modal actions ──────────────────────────────────────────────
    modal:                    Option<ModalDialog>,
    save_as_modal:            Option<SaveAsModal>,
    pending_error:            Option<String>,
    pending_rename:           Option<(PathBuf, String)>,
    pending_confirmed_delete: bool,
    pending_save_and_exit:    Option<bool>,
    show_help:                bool,
}

impl App {
    fn new(msg_tx: Sender<Msg>, msg_rx: Receiver<Msg>) -> Self {
        App {
            current_path:             None,
            current_image:            None,
            frame_index:              0,
            next_frame_at:            None,
            image_texture:            None,
            last_uploaded_gen:        u64::MAX,
            last_uploaded_frame:      usize::MAX,
            navigator:                DirNavigator::default(),
            msg_tx,
            msg_rx,
            loading_path:             None,
            preload_cache:            HashMap::new(),
            deleting:                 false,
            dir_watcher:              None,
            watched_dir:              None,
            fullscreen:               false,
            pending_fullscreen_toggle: false,
            maximized:                false,
            pre_maximize_size:        None,
            image_gen:                0,
            has_shown_first_image:    false,
            slideshow_active:  false,
            slideshow_next_at: None,
            zoom:          1.0,
            pan:           egui::Vec2::ZERO,
            show_guides:   false,
            guide_type:    GuideType::default(),
            metadata:      None,
            editor_state:             None,
            initial_editor_params:    None,
            editor_open_sections:     [true, true, false, false, false, false],
            thumb_cache:              None,
            icon_cache:               None,
            edit_rx:                  None,
            edit_gen:                 0,
            last_slider_change:       None,
            modal:                    None,
            save_as_modal:            None,
            pending_error:            None,
            pending_rename:           None,
            pending_confirmed_delete: false,
            pending_save_and_exit:    None,
            show_help:                false,
        }
    }

    fn in_editor(&self) -> bool { self.editor_state.is_some() }

    /// Returns the image to display: editor preview if in editor, else raw image.
    fn view(&self) -> Option<&DecodedImage> {
        self.editor_state.as_ref().map(|es| &es.display)
            .or(self.current_image.as_ref())
    }

    fn current_filename(&self) -> Option<String> {
        self.current_path.as_deref()
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
        if self.loading_path.as_deref() == Some(&path) { return; }
        self.loading_path = Some(path.clone());

        let tx  = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match load_image(&path) {
                LoadOutcome::Ok(img)                  => Msg::Loaded { path, image: img, mode },
                LoadOutcome::WrongExtension(img, ext) => Msg::WrongExtension { path, image: img, mode, correct_ext: ext },
                LoadOutcome::Unsupported(reason)      => Msg::Unsupported { path, reason },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn apply_image(&mut self, path: PathBuf, img: DecodedImage, mode: OpenMode, ctx: &egui::Context) {
        self.loading_path = None;
        self.deleting     = false;
        self.frame_index  = 0;
        self.metadata     = None;
        self.zoom         = 1.0;
        self.pan          = egui::Vec2::ZERO;

        // Schedule first animation frame for animated images.
        self.next_frame_at = if img.frames.len() > 1 {
            Some(Instant::now() + Duration::from_millis(img.frame(0).delay_ms as u64))
        } else { None };

        let dir_changed = self.current_path.as_deref().and_then(|p| p.parent())
            != path.parent();

        self.current_image = Some(img.clone());
        self.current_path  = Some(path.clone());
        self.image_gen    += 1;

        if dir_changed || mode == OpenMode::FirstImage {
            self.navigator.set_current(path.clone());
            self.watch_directory(path.parent().map(|p| p.to_path_buf()), ctx);
        }

        self.preload_neighbour(true,  ctx);
        self.preload_neighbour(false, ctx);

        // Load EXIF asynchronously — never blocks navigation.
        let meta_path = path.clone();
        let tx_meta   = self.msg_tx.clone();
        let ctx2      = ctx.clone();
        std::thread::spawn(move || {
            let meta = exif::read_metadata(&meta_path);
            let _ = tx_meta.send(Msg::MetadataLoaded { path: meta_path, meta });
            ctx2.request_repaint();
        });
    }

    fn preload_neighbour(&mut self, forward: bool, ctx: &egui::Context) {
        let target = if forward { self.navigator.next() } else { self.navigator.prev() };
        let Some(p) = target else { return };
        if self.preload_cache.contains_key(&p) { return; }
        if self.loading_path.as_deref() == Some(&p) { return; }

        let tx  = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let LoadOutcome::Ok(img) = load_image(&p) {
                let _ = tx.send(Msg::Preloaded { path: p, image: img });
                ctx.request_repaint();
            }
        });
    }

    fn watch_directory(&mut self, dir: Option<PathBuf>, ctx: &egui::Context) {
        let Some(dir) = dir else { return };
        if self.watched_dir.as_deref() == Some(&dir) { return; }
        self.watched_dir = Some(dir.clone());

        let tx  = self.msg_tx.clone();
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
        self.current_path  = None;
        self.frame_index   = 0;
        self.next_frame_at = None;
        self.image_gen    += 1;
        self.metadata      = None;
        self.zoom          = 1.0;
        self.pan           = egui::Vec2::ZERO;
    }

    // ── Animation ─────────────────────────────────────────────────────────────

    fn tick_animation(&mut self) -> bool {
        let Some(at) = self.next_frame_at else { return false };
        if Instant::now() < at { return false; }
        let Some(img) = &self.current_image else { self.next_frame_at = None; return false };
        let n = img.frames.len();
        if n <= 1 { self.next_frame_at = None; return false; }

        self.frame_index   = (self.frame_index + 1) % n;
        let delay          = img.frame(self.frame_index).delay_ms;
        self.next_frame_at = Some(Instant::now() + Duration::from_millis(delay as u64));
        self.image_gen    += 1;
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
                    if mode == OpenMode::FirstImage { dr.resize_to = Some((w, h)); }
                }
                Msg::WrongExtension { path, image, mode, correct_ext } => {
                    let (w, h) = (image.width, image.height);
                    self.apply_image(path.clone(), image, mode, ctx);
                    if mode == OpenMode::FirstImage { dr.resize_to = Some((w, h)); }
                    dr.rename_offer = Some((path, correct_ext));
                }
                Msg::Unsupported { path, reason } => {
                    log!("[load] unsupported: {}: {}", path.display(), reason);
                    dr.error_msg = Some(format!("Cannot open {}\n\n{reason}", path.display()));
                }
                Msg::Preloaded { path, image } => {
                    self.preload_cache.insert(path, image);
                }
                Msg::Deleted { path } => {
                    log!("[delete] done: {}", path.display());
                    self.deleting = false;
                    self.navigator.remove(&path);
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
        let next = if forward { self.navigator.next() } else { self.navigator.prev() };
        if let Some(p) = next { self.open_image(p, OpenMode::Navigate, ctx); }
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
        if self.current_image.is_none() { return; }
        self.slideshow_active  = true;
        self.slideshow_next_at = Some(Instant::now() + SLIDESHOW_INTERVAL);
        // Enter fullscreen for the slideshow experience.
        if !self.fullscreen { self.toggle_fullscreen(ctx); }
        ctx.request_repaint();
    }

    fn stop_slideshow(&mut self, ctx: &egui::Context) {
        self.slideshow_active  = false;
        self.slideshow_next_at = None;
        // Exit fullscreen when slideshow ends.
        if self.fullscreen { self.toggle_fullscreen(ctx); }
    }

    fn toggle_slideshow(&mut self, ctx: &egui::Context) {
        if self.slideshow_active { self.stop_slideshow(ctx); }
        else                     { self.start_slideshow(ctx); }
    }

    // ── Editor lifecycle ──────────────────────────────────────────────────────

    fn enter_editor(&mut self, ctx: &egui::Context) {
        if self.editor_state.is_some() { return; }
        // Clone the image out first — we need &mut self for stop_slideshow
        // and EditorState::new, which conflicts with borrowing current_image.
        let Some(img) = self.current_image.clone() else { return };
        if self.slideshow_active { self.stop_slideshow(ctx); }
        self.editor_state          = Some(EditorState::new(&img));
        self.initial_editor_params = Some(EditParams::default());
        self.thumb_cache           = Some(ThumbCache::new());
    }

    /// Returns `true` if the editor was exited or was already closed.
    fn exit_editor(&mut self, quit_intent: bool) -> bool {
        if self.editor_state.is_none() { return true; }
        if self.editor_params_changed() && self.modal.is_none() {
            self.modal = Some(ModalDialog::SaveOnExit { save_and_quit: quit_intent });
            return false;
        }
        self.do_exit_editor();
        true
    }

    fn do_exit_editor(&mut self) {
        self.editor_state          = None;
        self.initial_editor_params = None;
        self.thumb_cache           = None;
        self.icon_cache            = None;
        self.edit_rx               = None;
        self.last_slider_change    = None;
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
        debug_assert!(self.in_editor(), "editor_params_mut called outside editor mode");
        let mut p = self.editor_state.as_ref()
            .map(|es| es.undo.current().clone())
            .unwrap_or_default();
        f(&mut p);
        p
    }

    fn editor_undo(&mut self) { self.editor_step(|u| u.undo()); }
    fn editor_redo(&mut self) { self.editor_step(|u| u.redo()); }

    fn editor_step(&mut self, f: impl FnOnce(&mut editor::UndoStack)) {
        if let Some(es) = &mut self.editor_state { f(&mut es.undo); es.dirty = true; }
        if let Some(p) = self.editor_state.as_ref().map(|es| es.undo.current().clone()) {
            self.launch_async_edit(p);
        }
    }

    /// Commit params, cancel debounce, and immediately launch async re-encode.
    fn editor_commit_and_sync(&mut self, p: EditParams) {
        self.last_slider_change = None;
        if let Some(es) = &mut self.editor_state { es.commit(p.clone()); }
        self.launch_async_edit(p);
    }

    fn launch_async_edit(&mut self, params: EditParams) {
        let Some(orig) = self.current_image.clone() else { return };
        self.edit_gen += 1;
        let gen       = self.edit_gen;
        let (tx, rx)  = bounded(1);
        self.edit_rx  = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send((gen, apply_edits(&orig, &params)));
        });
    }

    fn poll_edit_result(&mut self) -> bool {
        let Some(rx) = &self.edit_rx else { return false };
        match rx.try_recv() {
            Ok((gen, img)) => {
                self.edit_rx = None;
                if gen == self.edit_gen {
                    if let Some(es) = &mut self.editor_state {
                        es.display = img;
                        es.dirty   = false;
                        self.image_gen += 1;
                    }
                    return true;
                }
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty)        => false,
            Err(crossbeam_channel::TryRecvError::Disconnected) => { self.edit_rx = None; false }
        }
    }

    // ── Panel action dispatch ─────────────────────────────────────────────────

    fn apply_panel_action(&mut self, action: PanelAction, ctx: &egui::Context) {
        // Any explicit action cancels pending debounced slider re-encode.
        if action.has_explicit_edit() { self.last_slider_change = None; }

        if action.save    { log!("[action] Save");    self.save_inplace(ctx);     return; }
        if action.save_as { log!("[action] SaveAs");  self.save_as_returning(ctx); return; }
        if action.undo    { log!("[action] Undo");    self.editor_undo();          return; }
        if action.redo    { log!("[action] Redo");    self.editor_redo();          return; }

        // Geometry actions: dispatch without heap allocation.
        enum Geom { ResetAll, RotCw, RotCcw, FlipH, FlipV }
        let geom = if      action.reset_all { Some(Geom::ResetAll) }
            else if action.rot_cw           { Some(Geom::RotCw)    }
            else if action.rot_ccw          { Some(Geom::RotCcw)   }
            else if action.flip_h           { Some(Geom::FlipH)    }
            else if action.flip_v           { Some(Geom::FlipV)    }
            else                            { None                  };

        if let Some(g) = geom {
            let p = match g {
                Geom::ResetAll => { log!("[action] ResetAll"); EditParams::default() }
                Geom::RotCw  => { log!("[action] RotCw");  self.editor_params_mut(|p| p.rotation = p.rotation.cw())  }
                Geom::RotCcw => { log!("[action] RotCcw"); self.editor_params_mut(|p| p.rotation = p.rotation.ccw()) }
                Geom::FlipH  => { log!("[action] FlipH");  self.editor_params_mut(|p| p.flip_h = !p.flip_h)          }
                Geom::FlipV  => { log!("[action] FlipV");  self.editor_params_mut(|p| p.flip_v = !p.flip_v)          }
            };
            self.editor_commit_and_sync(p);
            return;
        }

        // Slider / preset change: debounce before re-encoding.
        if let Some(new_p) = action.new_params {
            log!("[action] ParamChange");
            if let Some(es) = &mut self.editor_state { es.commit(new_p); }
            self.last_slider_change = Some(Instant::now());
        }
    }

    // ── Save / delete ─────────────────────────────────────────────────────────

    fn save_inplace(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current_path.clone() else { return };
        let Some(es)   = &self.editor_state         else { return };
        let params     = es.undo.current().clone();

        // Attempt lossless JPEG transform when only geometry changed.
        // This avoids a full decode→process→encode cycle and preserves original quality.
        if is_pure_geometry(&params) && is_jpeg_path(&path) {
            match lossless_jpeg_transform(&path, &params) {
                Ok(true) => {
                    log!("[save] lossless JPEG transform applied");
                    update_title(ctx, self.current_filename().as_deref());
                    self.reset_editor_after_save();
                    return;
                }
                Ok(false) => {
                    log!("[save] lossless not applicable (non-MCU-aligned), re-encoding");
                }
                Err(e) => {
                    self.pending_error = Some(format!("Save failed:\n{e}"));
                    return;
                }
            }
        }

        if let Err(e) = save_image_to_path(&es.display, &path, self.frame_index) {
            self.pending_error = Some(format!("Save failed:\n{e}"));
        } else {
            update_title(ctx, self.current_filename().as_deref());
            self.reset_editor_after_save();
        }
    }

    /// Opens the custom Save As modal; actual write happens on confirm.
    fn save_as_returning(&mut self, _ctx: &egui::Context) -> bool {
        if self.editor_state.is_none() { return false; }
        let stem = self.current_path.as_deref()
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}_edited", s.to_string_lossy()))
            .unwrap_or_else(|| "edited".into());
        let format    = self.current_path.as_deref()
            .map(OutputFormat::from_path).unwrap_or(OutputFormat::Png);
        let directory = self.current_path.as_deref()
            .and_then(|p| p.parent()).unwrap_or(Path::new(".")).to_path_buf();
        self.save_as_modal = Some(SaveAsModal { stem, format, directory });
        true
    }

    fn commit_save_as(&mut self, ctx: &egui::Context, dest: PathBuf) {
        let Some(es) = &self.editor_state else { return };
        let display  = es.display.clone();
        if let Err(e) = save_image_to_path(&display, &dest, self.frame_index) {
            self.pending_error = Some(format!("Save failed:\n{e}"));
            return;
        }
        self.current_path  = Some(dest.clone());
        self.current_image = Some(display);
        self.navigator.set_current(dest);
        update_title(ctx, self.current_filename().as_deref());
        self.image_gen += 1;
        self.reset_editor_after_save();
    }

    fn reset_editor_after_save(&mut self) {
        if let Some(es) = &mut self.editor_state {
            es.undo.push(EditParams::default());
            es.dirty = false;
        }
        self.initial_editor_params = Some(EditParams::default());
    }

    fn delete_current(&mut self) {
        let Some(path) = &self.current_path else { return };
        if self.deleting { return; }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("this file");
        self.modal = Some(ModalDialog::ConfirmDelete { filename: fname.to_string() });
    }

    fn execute_delete(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current_path.clone() else { return };
        if self.deleting { return; }
        self.deleting = true;

        // Always exit editor mode on delete — the file is going away.
        if self.in_editor() { self.do_exit_editor(); }

        // Navigate away before deleting so display doesn't go blank.
        let next = self.navigator.next().filter(|p| p != &path)
            .or_else(|| self.navigator.prev().filter(|p| p != &path));
        if let Some(np) = next { self.open_image(np, OpenMode::Navigate, ctx); }
        else                   { self.clear_image(); }

        let tx  = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = std::fs::remove_file(&path);
            let _ = tx.send(Msg::Deleted { path });
            ctx.request_repaint();
        });
    }

} // end impl App

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.apply_deferred_viewport(ctx, frame);
        self.process_background_messages(ctx);
        self.tick_and_schedule_animation(ctx);
        self.drive_async_edit(ctx);
        self.upload_current_frame(ctx, frame);
        self.draw_ui(ctx);
        self.process_keyboard(ctx);
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
        if let Some(quit_after) = self.pending_save_and_exit.take() {
            self.save_inplace(ctx);
            self.do_exit_editor();
            if quit_after { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }
        if self.pending_confirmed_delete {
            self.pending_confirmed_delete = false;
            self.execute_delete(ctx);
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
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                    egui::vec2(cw as f32, ch as f32)));
                if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
                    ctx.send_viewport_cmd(cmd);
                }
            }
        }
        if let Some(msg) = dr.error_msg           { self.pending_error  = Some(msg); }
        if let Some(offer) = dr.rename_offer       { self.pending_rename = Some(offer); }
    }

    /// Advance animation by one frame if the timer has elapsed.
    fn tick_and_schedule_animation(&mut self, ctx: &egui::Context) {
        if self.tick_animation() { ctx.request_repaint(); }
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
                    self.slideshow_next_at.unwrap().saturating_duration_since(Instant::now()));
            }
        }
    }

    /// Drive the debounced slider pipeline and poll for completed edits.
    fn drive_async_edit(&mut self, ctx: &egui::Context) {
        if self.last_slider_change.is_some_and(|t| t.elapsed().as_millis() >= SLIDER_DEBOUNCE_MS) {
            self.last_slider_change = None;
            if let Some(p) = self.editor_state.as_ref().map(|es| es.undo.current().clone()) {
                log!("[debounce] firing async edit after slider idle");
                self.launch_async_edit(p);
            }
        } else if self.last_slider_change.is_some() || self.edit_rx.is_some() {
            ctx.request_repaint();
        }
        if self.poll_edit_result() { ctx.request_repaint(); }
    }

    /// Upload pixel data to the GPU if the image generation or frame changed.
    fn upload_current_frame(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.image_gen == self.last_uploaded_gen
            && self.frame_index == self.last_uploaded_frame
        { return; }

        self.last_uploaded_gen   = self.image_gen;
        self.last_uploaded_frame = self.frame_index;

        // Clone pixels before the mutable borrow on image_texture.
        let upload = self.view().map(|img| {
            let fd = img.frame(self.frame_index);
            (fd.pixels.clone(), img.width, img.height)
        });
        if let Some((pixels, w, h)) = upload {
            let t = Instant::now();
            upload_image_texture(ctx, frame, &pixels, w, h, &mut self.image_texture);
            let ms = t.elapsed().as_millis();
            if ms > 2 { log!("[upload] {}×{} {}ms", w, h, ms); }
        }
    }

    /// Build the egui UI for this frame.
    fn draw_ui(&mut self, ctx: &egui::Context) {
        ui::apply_panel_style(ctx);
        self.promote_pending_modals(ctx);
        self.ensure_editor_caches(ctx);

        // Modals overlay everything — render before panels.
        let icons_ptr = self.icon_cache.as_ref().map(|ic| ic as *const ui::Icons);
        // SAFETY: icon_cache is never mutated inside draw_modal or draw_save_as_modal.
        let icons_ref = icons_ptr.map(|p| unsafe { &*p });
        draw_modal(ctx, self, icons_ref);
        draw_save_as_modal(ctx, self);
        if self.show_help { draw_help_modal(ctx, self.in_editor(), &mut self.show_help); }

        // SidePanel must precede CentralPanel so egui allocates its input region first.
        let pending_action = self.draw_editor_panel(ctx);

        self.draw_canvas(ctx);

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
                let old_ext = old_path.extension()
                    .and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                self.modal = Some(ModalDialog::OfferRename {
                    old_path, new_path, old_ext, new_ext: correct_ext,
                });
            }
        }
    }

    /// Lazily initialise icon and thumbnail caches when the editor is open.
    fn ensure_editor_caches(&mut self, ctx: &egui::Context) {
        if !self.in_editor() { return; }
        if let Some(tc) = &mut self.thumb_cache { tc.upload(ctx); }
        if self.icon_cache.is_none() { self.icon_cache = Some(Icons::load(ctx)); }
    }

    /// Draw the editor side panel; returns any pending action for this frame.
    fn draw_editor_panel(&mut self, ctx: &egui::Context) -> Option<PanelAction> {
        if !self.in_editor() { return None; }
        let params   = self.editor_state.as_ref().map(|es| es.undo.current().clone()).unwrap_or_default();
        let can_undo = self.editor_state.as_ref().map(|es| es.undo.can_undo()).unwrap_or(false);
        let can_redo = self.editor_state.as_ref().map(|es| es.undo.can_redo()).unwrap_or(false);
        let meta_ref = self.metadata.as_ref();
        let action   = draw_panel(
            ctx, &params, can_undo, can_redo,
            self.thumb_cache.as_ref().unwrap(),
            self.icon_cache.as_ref().unwrap(),
            &mut self.editor_open_sections,
            meta_ref.map(|m| m as &crate::exif::ImageMetadata),
        );
        if action.has_any() { Some(action) } else { None }
    }

    /// Draw the canvas (CentralPanel): image, scroll navigation, drag-and-drop.
    fn draw_canvas(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(CANVAS_COLOR))
            .show(ctx, |ui| {
                self.draw_canvas_image(ui);
                self.handle_canvas_scroll(ui, ctx);
                self.handle_canvas_drop(ui, ctx);
            });
    }

    fn draw_canvas_image(&mut self, ui: &mut egui::Ui) {
        let Some(it) = &self.image_texture else { return };
        let canvas   = ui.available_rect_before_wrap();
        let scale    = self.editor_state.as_ref()
            .map(|es| es.undo.current().scale).unwrap_or(1.0);

        // Allocate the full canvas first for reliable click/drag sense.
        let canvas_resp = ui.allocate_rect(canvas, egui::Sense::click_and_drag());

        // Fit image to canvas, apply edit scale, then apply view zoom.
        let fit = (canvas.width()  / it.width  as f32)
            .min(canvas.height() / it.height as f32);
        let base_w = it.width  as f32 * fit * scale;
        let base_h = it.height as f32 * fit * scale;
        // Clamp edit scale within canvas, then apply view zoom on top.
        let edit_clamp = (canvas.width() / base_w).min(canvas.height() / base_h).min(1.0);
        let dw = base_w * edit_clamp * self.zoom;
        let dh = base_h * edit_clamp * self.zoom;

        // Centre + apply pan offset. In editor mode, zoom and pan are active.
        let cx = canvas.min.x + (canvas.width()  - dw) * 0.5;
        let cy = canvas.min.y + (canvas.height() - dh) * 0.5;
        let (ox, oy) = if self.in_editor() {
            (cx + self.pan.x, cy + self.pan.y)
        } else {
            (cx, cy)
        };

        let img_rect = egui::Rect::from_min_size(
            egui::pos2(ox, oy), egui::vec2(dw, dh));

        egui::Image::new(egui::load::SizedTexture::new(
            it.texture_id, egui::vec2(it.width as f32, it.height as f32),
        ))
        .fit_to_exact_size(egui::vec2(dw, dh))
        .paint_at(ui, img_rect);

        // ── Guide overlays (editor mode only) ─────────────────────────────────
        if self.in_editor() && self.show_guides {
            draw_guides(ui, img_rect, self.guide_type);
        }

        // ── Pan (drag in editor mode) ──────────────────────────────────────────
        if self.in_editor() && canvas_resp.dragged() {
            self.pan += canvas_resp.drag_delta();
        }

        // ── Double-click: toggle maximize (when not dragging) ─────────────────
        if canvas_resp.double_clicked() { self.toggle_maximize(ui.ctx()); }
    }

    fn handle_canvas_scroll(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (scroll_y, ctrl_held) = ui.input_mut(|i| {
            let dir = i.events.iter().find_map(|ev| {
                if let egui::Event::Scroll(d) = ev {
                    if d.y.abs() > d.x.abs() && d.y != 0.0 { return Some(d.y); }
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
            if !self.in_editor() { self.open_image(path, OpenMode::Navigate, ctx); }
        }
    }

    /// Handle all keyboard shortcuts for this frame using `consume_key`.
    ///
    /// `consume_key` bypasses egui's "consumed" flag, ensuring app shortcuts
    /// fire even when widgets have focus. All shortcuts are polled every frame.
    fn process_keyboard(&mut self, ctx: &egui::Context) {
        let no   = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers::COMMAND;
        let cs   = ctrl | egui::Modifiers::SHIFT;

        macro_rules! key {
            ($mods:expr, $k:expr) => { ctx.input_mut(|i| i.consume_key($mods, $k)) };
        }

        if self.in_editor() {
            if key!(ctrl, egui::Key::Z)       { self.editor_undo(); }
            if key!(cs,   egui::Key::Z)        { self.editor_redo(); }
            if key!(ctrl, egui::Key::Y)        { self.editor_redo(); }
            if key!(ctrl, egui::Key::S)        { self.save_inplace(ctx); }
            if key!(cs,   egui::Key::S)        { self.save_as_returning(ctx); }
            if key!(no,   egui::Key::E)        { self.exit_editor(false); }
            if key!(no,   egui::Key::Escape)   { self.exit_editor(false); }
            if key!(no,   egui::Key::F)        { self.toggle_fullscreen(ctx); }
            if key!(no,   egui::Key::F11)      { self.toggle_fullscreen(ctx); }
            if key!(no,   egui::Key::H)        { self.show_help = !self.show_help; }
            if key!(no,   egui::Key::Delete)   { self.delete_current(); }
            if key!(no,   egui::Key::G)            { self.show_guides = !self.show_guides; }
            if key!(no,   egui::Key::Tab) {
                // Cycle guide type when guides are visible
                self.guide_type = match self.guide_type {
                    GuideType::RuleOfThirds => GuideType::GoldenRatio,
                    GuideType::GoldenRatio  => GuideType::RuleOfThirds,
                };
            }
            if key!(ctrl, egui::Key::Num0) || key!(ctrl, egui::Key::Space) {
                // Reset zoom and pan
                self.zoom = 1.0;
                self.pan  = egui::Vec2::ZERO;
            }
            if key!(no,   egui::Key::Q) {
                if self.exit_editor(true) { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
            }
        } else {
            if key!(no, egui::Key::ArrowRight) { self.navigate(true,  ctx); }
            if key!(no, egui::Key::ArrowDown)  { self.navigate(true,  ctx); }
            if key!(no, egui::Key::PageDown)   { self.navigate(true,  ctx); }
            if key!(no, egui::Key::ArrowLeft)  { self.navigate(false, ctx); }
            if key!(no, egui::Key::ArrowUp)    { self.navigate(false, ctx); }
            if key!(no, egui::Key::PageUp)     { self.navigate(false, ctx); }
            if key!(no, egui::Key::E) {
                self.enter_editor(ctx);
                if self.thumb_cache.is_none() { self.thumb_cache = Some(ThumbCache::new()); }
            }
            if key!(no, egui::Key::F) || key!(no, egui::Key::F11) {
                // F/F11 in slideshow: exit slideshow (which also exits fullscreen).
                // F/F11 outside slideshow: toggle fullscreen normally.
                if self.slideshow_active { self.stop_slideshow(ctx); }
                else                     { self.toggle_fullscreen(ctx); }
            }
            if key!(no, egui::Key::Escape) {
                // Stop slideshow (which also exits fullscreen via stop_slideshow).
                // If not in slideshow but in fullscreen, exit fullscreen directly.
                if self.slideshow_active { self.stop_slideshow(ctx); }
                else if self.fullscreen  { self.toggle_fullscreen(ctx); }
            }
            if key!(no, egui::Key::S)          { self.toggle_slideshow(ctx); }
            if key!(no, egui::Key::H)          { self.show_help = !self.show_help; }
            if key!(no, egui::Key::Delete)     { self.delete_current(); }
            if key!(no, egui::Key::Q)          { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }
    }
}

// ── Help modal ────────────────────────────────────────────────────────────────

fn draw_help_modal(ctx: &egui::Context, in_editor: bool, show: &mut bool) {
    use ui::{BG, BORDER, TEXT, DIM, ACCENT, FAINT};

    ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("help_dim")))
        .rect_filled(ctx.screen_rect(), 0.0, egui::Color32::from_black_alpha(160));

    egui::Window::new("help_modal")
        .title_bar(false).resizable(false).collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::none().fill(BG)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(24.0))
        .show(ctx, |ui| {
            ui.set_width(480.0);
            ui.visuals_mut().override_text_color               = None;
            ui.visuals_mut().widgets.noninteractive.fg_stroke.color = TEXT;

            ui.label(egui::RichText::new("Keyboard Shortcuts").color(TEXT).size(16.0).strong());
            ui.add_space(12.0);

            // Two-column layout: key | description
            let row = |ui: &mut egui::Ui, key: &str, desc: &str| {
                ui.horizontal(|ui| {
                    let (_, kr) = ui.allocate_exact_size(
                        egui::vec2(120.0, 22.0), egui::Sense::hover());
                    ui.painter().text(kr.rect.left_center(), egui::Align2::LEFT_CENTER,
                        key, egui::FontId::monospace(12.0), ACCENT);
                    ui.label(egui::RichText::new(desc).color(TEXT).size(12.0));
                });
            };
            let section = |ui: &mut egui::Ui, title: &str| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(title).color(FAINT).size(10.5).strong());
                ui.add_space(2.0);
            };

            section(ui, "VIEWER");
            row(ui, "← ↑ PgUp",       "Previous image");
            row(ui, "→ ↓ PgDn",       "Next image");
            row(ui, "E",               "Enter Editor mode");
            row(ui, "S",               "Start / stop slideshow");
            row(ui, "F  /  F11",       "Toggle fullscreen");
            row(ui, "Del",             "Delete current image");
            row(ui, "Q",               "Quit");

            section(ui, "SLIDESHOW");
            row(ui, "S",               "Stop slideshow");
            row(ui, "Esc",             "Stop slideshow");
            row(ui, "F  /  F11",       "Stop slideshow");

            if in_editor {
                section(ui, "EDITOR");
                row(ui, "E  /  Esc",       "Return to Viewer");
                row(ui, "Ctrl+Z",          "Undo");
                row(ui, "Ctrl+Shift+Z",    "Redo");
                row(ui, "Ctrl+S",          "Save");
                row(ui, "Ctrl+Shift+S",    "Save As");
                row(ui, "Del",             "Delete current image");
                row(ui, "G",               "Toggle guide overlay");
                row(ui, "Tab",             "Cycle guide type");
                row(ui, "Ctrl+Scroll",     "Zoom in / out");
                row(ui, "Ctrl+0",          "Reset zoom & pan");
                row(ui, "Drag canvas",     "Pan");
                row(ui, "F  /  F11",       "Toggle fullscreen");
                row(ui, "Q",               "Quit");
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Press H or Esc to close").color(DIM).size(11.0));
        });

    if ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) ||
        i.consume_key(egui::Modifiers::NONE, egui::Key::H)
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
fn draw_guides(ui: &egui::Ui, r: egui::Rect, guide_type: GuideType) {
    const LINE_COLOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(255, 255, 255, 60);
    const STROKE: egui::Stroke = egui::Stroke { width: 1.0, color: LINE_COLOR };
    let p = ui.painter();

    let divisions: &[f32] = match guide_type {
        // Rule of thirds: divide into equal thirds
        GuideType::RuleOfThirds => &[1.0/3.0, 2.0/3.0],
        // Golden ratio: phi-based divisions (~0.382 / 0.618)
        GuideType::GoldenRatio  => &[0.382, 0.618],
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

fn image_copy(texture: &wgpu::Texture) -> wgpu::ImageCopyTexture<'_> {
    wgpu::ImageCopyTexture {
        texture, mip_level: 0,
        origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
    }
}

fn upload_image_texture(
    _ctx:   &egui::Context,
    frame:  &mut eframe::Frame,
    pixels: &[u8],
    w:      u32,
    h:      u32,
    slot:   &mut Option<ImageTexture>,
) {
    if w == 0 || h == 0 { return; }
    let Some(rs) = frame.wgpu_render_state() else {
        eprintln!("[upload] wgpu render state unavailable");
        return;
    };

    let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let layout = wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) };
    // In-place update when dimensions match.
    if let Some(it) = slot.as_ref().filter(|it| it.width == w && it.height == h) {
        rs.queue.write_texture(image_copy(&it.texture), pixels, layout, extent);
        return;
    }

    // Unregister the old texture if dimensions changed.
    if let Some(it) = slot.take() {
        rs.renderer.write().free_texture(&it.texture_id);
    }

    let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("patina_image"), size: extent, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    rs.queue.write_texture(image_copy(&texture), pixels, layout, extent);

    let view       = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let texture_id = rs.renderer.write()
        .register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);

    *slot = Some(ImageTexture { texture, texture_id, width: w, height: h });
}

// ── Modal button painter ──────────────────────────────────────────────────────
//
// Single implementation shared by both `draw_modal` and `draw_save_as_modal`.
// All modal buttons use the same layout, paint logic, and colour vocabulary.

#[derive(Clone, Copy)]
#[allow(dead_code)] // `Danger` reserved for future destructive confirmation dialogs
enum ModalBtnKind { Primary, Default, Danger }

fn modal_btn(
    ui:     &mut egui::Ui,
    label:  &str,
    icon:   Option<&egui::TextureHandle>,
    kind:   ModalBtnKind,
) -> egui::Response {
    use egui::{Rounding, Sense, Stroke, Vec2};
    use ui::{ACCENT, ACCENT_BG, BORDER, BTN, ON_PRIMARY, TEXT, DANGER, DANGER_BG, DANGER_BDR, DANGER_PR};
    use ui::ACCENT_DIM; // pressed-state for primary

    // Shared layout constants.
    const PAD:     f32 = 12.0;
    const GAP:     f32 = 6.0;
    const ICON_SZ: f32 = 14.0;
    const FONT:    f32 = 13.0;
    const HEIGHT:  f32 = 32.0;
    const RADIUS:  f32 = 5.0;
    const HOVER_PRIMARY: egui::Color32 = egui::Color32::from_rgb(0x8D, 0xAF, 0xA0);
    const PRESS_DEFAULT: egui::Color32 = egui::Color32::from_rgba_premultiplied(35, 68, 59, 56);
    const HOVER_BDR:     egui::Color32 = egui::Color32::from_rgba_premultiplied(71, 86, 79, 115);

    let fg_base = if matches!(kind, ModalBtnKind::Primary) { ON_PRIMARY } else { TEXT };
    let ic_w    = if icon.is_some() { ICON_SZ + GAP } else { 0.0 };
    let lbl_w   = ui.fonts(|f| f.layout_no_wrap(
        label.to_string(), egui::FontId::proportional(FONT), fg_base).size().x);
    let w = (PAD + ic_w + lbl_w + PAD).max(72.0);

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, HEIGHT), Sense::click());
    let pt   = ui.painter();
    let r    = Rounding::same(RADIUS);
    let down = resp.is_pointer_button_down_on();
    let hov  = resp.hovered();

    let (bg, fg, border) = match kind {
        ModalBtnKind::Primary => {
            let bg = if down { ACCENT_DIM } else if hov { HOVER_PRIMARY } else { ACCENT };
            (bg, ON_PRIMARY, None)
        }
        ModalBtnKind::Default => {
            let (bg, fg, bdr) = if down { (PRESS_DEFAULT, ACCENT, HOVER_BDR) }
                else if hov             { (ACCENT_BG,     ACCENT, HOVER_BDR) }
                else                    { (BTN,           TEXT,   BORDER)    };
            (bg, fg, Some((1.0f32, bdr)))
        }
        ModalBtnKind::Danger => {
            let (bg, fg, bdr) = if down { (DANGER_PR, DANGER, DANGER_BDR) }
                else if hov             { (DANGER_BG, DANGER, DANGER_BDR) }
                else                    { (BTN,       TEXT,   BORDER)     };
            (bg, fg, Some((1.0f32, bdr)))
        }
    };

    pt.rect_filled(rect, r, bg);
    if let Some((sw, sc)) = border { pt.rect_stroke(rect, r, Stroke::new(sw, sc)); }

    let mut x = rect.min.x + PAD;
    if let Some(tex) = icon {
        let ir = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - ICON_SZ * 0.5), Vec2::splat(ICON_SZ));
        pt.image(tex.id(), ir,
            egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)), fg);
        x += ICON_SZ + GAP;
    }
    pt.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER,
        label, egui::FontId::proportional(FONT), fg);
    resp
}

// ── Save As modal ─────────────────────────────────────────────────────────────

fn draw_save_as_modal(ctx: &egui::Context, app: &mut App) {
    let Some(ref mut modal) = app.save_as_modal else { return };
    use ui::{BG, BORDER, DIM, TEXT};

    // Dim at Background so the Window (Middle) is always above it.
    ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("saveas_dim")))
        .rect_filled(ctx.screen_rect(), 0.0, egui::Color32::from_black_alpha(160));

    let mut confirm = false;
    let mut cancel  = false;
    let mut browse  = false;

    egui::Window::new("save_as_modal")
        .title_bar(false).resizable(false).collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::none().fill(BG)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(24.0))
        .show(ctx, |ui| {
            ui.set_width(360.0);
            // Reset global style overrides so modal widgets render at full opacity.
            ui.visuals_mut().override_text_color               = None;
            ui.visuals_mut().widgets.noninteractive.fg_stroke.color = TEXT;
            ui.visuals_mut().widgets.inactive.fg_stroke.color       = TEXT;

            ui.label(egui::RichText::new("Save As").color(TEXT).size(16.0).strong());
            ui.add_space(16.0);

            // Folder row
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Folder").color(DIM).size(12.0));
                ui.add_space(4.0);
                let dir_str = modal.directory.to_string_lossy();
                let display = truncate_path_display(&dir_str, 38);
                ui.label(egui::RichText::new(display).color(TEXT).size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if modal_btn(ui, "Browse…", None, ModalBtnKind::Default).clicked() {
                        browse = true;
                    }
                });
            });
            ui.add_space(12.0);

            // Filename field
            ui.label(egui::RichText::new("Filename").color(DIM).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut modal.stem)
                    .desired_width(260.0).font(egui::FontId::proportional(13.0)));
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                ui.label(egui::RichText::new(
                    format!(".{}", modal.format.extension())).color(DIM).size(13.0));
            });
            ui.add_space(12.0);

            // Format selector
            ui.label(egui::RichText::new("Format").color(DIM).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for &fmt in OutputFormat::ALL {
                    if format_btn(ui, fmt.label(), modal.format == fmt).clicked() {
                        modal.format = fmt;
                    }
                }
            });
            ui.add_space(16.0);

            ui.label(egui::RichText::new(
                format!("→ {}", modal.full_path().display())).color(DIM).size(11.0));
            ui.add_space(16.0);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if modal_btn(ui, "Save",   None, ModalBtnKind::Primary).clicked() { confirm = true; }
                if modal_btn(ui, "Cancel", None, ModalBtnKind::Default).clicked() { cancel  = true; }
            });
        });

    if browse {
        let start = app.save_as_modal.as_ref().map(|m| m.directory.clone());
        if let Some(dir) = {
            let mut d = rfd::FileDialog::new().set_title("Choose folder");
            if let Some(s) = start { d = d.set_directory(s); }
            d.pick_folder()
        } {
            if let Some(m) = &mut app.save_as_modal { m.directory = dir; }
        }
    }
    if cancel  { app.save_as_modal = None; }
    if confirm {
        if let Some(m) = app.save_as_modal.take() {
            app.commit_save_as(ctx, m.full_path());
        }
    }
}

/// Paint a format selector pill button (active = spruce accent, inactive = dark with hover).
fn format_btn(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    use ui::{ACCENT, ACCENT_BG, BORDER, BTN, TEXT};
    const FONT:   f32 = 12.0;
    const HEIGHT: f32 = 26.0;
    const PAD:    f32 = 8.0;
    const HOVER_BDR: egui::Color32 = egui::Color32::from_rgba_premultiplied(71, 86, 79, 115);
    const PRESS_BG:  egui::Color32 = egui::Color32::from_rgba_premultiplied(35, 68, 59, 56);

    let lbl_col = if active { ACCENT } else { TEXT };
    let lbl_w   = ui.fonts(|f| f.layout_no_wrap(
        label.to_string(), egui::FontId::proportional(FONT), lbl_col).size().x);
    let (rect, resp) = ui.allocate_exact_size(
        egui::Vec2::new(lbl_w + PAD * 2.0, HEIGHT), egui::Sense::click());
    let p = ui.painter();
    let r = egui::Rounding::same(4.0);

    let (bg, fg, stroke_w) = if active {
        (ACCENT_BG, ACCENT, 1.5f32)
    } else if resp.is_pointer_button_down_on() {
        (PRESS_BG, ACCENT, 1.0)
    } else if resp.hovered() {
        (ACCENT_BG, ACCENT, 1.0)
    } else {
        (BTN, TEXT, 1.0)
    };
    let border_col = if active || resp.hovered() || resp.is_pointer_button_down_on() {
        HOVER_BDR
    } else { BORDER };

    p.rect_filled(rect, r, bg);
    p.rect_stroke(rect, r, egui::Stroke::new(stroke_w, border_col));
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(FONT), fg);
    resp
}

// ── Main modal ────────────────────────────────────────────────────────────────

fn draw_modal(ctx: &egui::Context, app: &mut App, icons: Option<&ui::Icons>) {
    let Some(ref modal) = app.modal else { return };
    use ui::{BG, BORDER, DANGER, DIM, TEXT};

    // Dim overlay at Order::Background so the Window (Order::Middle) renders above it.
    // Using Foreground here would paint over the Window, making it appear greyed out.
    ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("modal_dim")))
        .rect_filled(ctx.screen_rect(), 0.0, egui::Color32::from_black_alpha(160));

    let mut close  = false;
    let mut action: Option<ModalAction> = None;

    egui::Window::new("modal")
        .title_bar(false).resizable(false).collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::none().fill(BG)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(24.0))
        .show(ctx, |ui| {
            ui.set_width(340.0);
            // Reset global style overrides so modal text renders at full opacity.
            // apply_panel_style sets override_text_color and fg_stroke globally,
            // which bleeds into Window widgets and makes them appear disabled.
            ui.visuals_mut().override_text_color               = None;
            ui.visuals_mut().widgets.noninteractive.fg_stroke.color = TEXT;
            ui.visuals_mut().widgets.inactive.fg_stroke.color       = TEXT;

            let btn_row = |ui: &mut egui::Ui, f: &mut dyn FnMut(&mut egui::Ui)| {
                ui.add_space(16.0);
                ui.horizontal(|ui| { ui.spacing_mut().item_spacing.x = 8.0; f(ui); });
            };
            match modal {
                ModalDialog::SaveOnExit { .. } => {
                    ui.label(egui::RichText::new("Save changes?").color(TEXT).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Your edits will be lost if you don't save.").color(DIM).size(13.0));
                    btn_row(ui, &mut |ui| {
                        if modal_btn(ui, "Save",    icons.map(|ic| &ic.save), ModalBtnKind::Primary).clicked() { action = Some(ModalAction::Save);    close = true; }
                        if modal_btn(ui, "Discard", None,                      ModalBtnKind::Default).clicked() { action = Some(ModalAction::Discard); close = true; }
                        if modal_btn(ui, "Cancel",  None,                      ModalBtnKind::Default).clicked() { close = true; }
                    });
                }
                ModalDialog::ConfirmDelete { filename } => {
                    ui.label(egui::RichText::new("Delete image?").color(TEXT).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(
                        format!("Permanently delete\n\n{filename}?\n\nThis cannot be undone.")
                    ).color(DIM).size(13.0));
                    btn_row(ui, &mut |ui| {
                        if modal_btn(ui, "Delete", icons.map(|ic| &ic.reset), ModalBtnKind::Danger).clicked() { action = Some(ModalAction::Delete); close = true; }
                        if modal_btn(ui, "Cancel", None,                       ModalBtnKind::Default).clicked() { close = true; }
                    });
                }
                ModalDialog::OfferRename { old_ext, new_ext, new_path, .. } => {
                    ui.label(egui::RichText::new("Wrong file extension").color(TEXT).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(format!(
                        "This file is a .{new_ext} saved as .{old_ext}.\n\nRename to: {}",
                        new_path.file_name().unwrap_or_default().to_string_lossy()
                    )).color(DIM).size(13.0));
                    btn_row(ui, &mut |ui| {
                        if modal_btn(ui, "Rename", icons.map(|ic| &ic.save), ModalBtnKind::Primary).clicked() { action = Some(ModalAction::Rename); close = true; }
                        if modal_btn(ui, "Keep",   None,                      ModalBtnKind::Default).clicked() { close = true; }
                    });
                }
                ModalDialog::Error { message } => {
                    ui.label(egui::RichText::new("Error").color(DANGER).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(message.as_str()).color(TEXT).size(13.0));
                    btn_row(ui, &mut |ui| {
                        if modal_btn(ui, "OK", None, ModalBtnKind::Default).clicked() { close = true; }
                    });
                }
            }
        });

    if close {
        if let Some(a) = action { apply_modal_action(app, a); }
        app.modal = None;
    }
}

/// Type-safe modal action — replaces the stringly-typed `&str` approach.
enum ModalAction { Save, Discard, Delete, Rename }

fn apply_modal_action(app: &mut App, action: ModalAction) {
    match action {
        ModalAction::Save => {
            if let Some(ModalDialog::SaveOnExit { save_and_quit }) = &app.modal {
                app.pending_save_and_exit = Some(*save_and_quit);
            }
        }
        ModalAction::Discard => { app.do_exit_editor(); }
        ModalAction::Delete  => { app.pending_confirmed_delete = true; }
        ModalAction::Rename  => {
            if let Some(ModalDialog::OfferRename { old_path, new_path, .. }) = &app.modal {
                let (old, new) = (old_path.clone(), new_path.clone());
                if std::fs::rename(&old, &new).is_ok() {
                    app.navigator.rename(&old, new.clone());
                    app.current_path = Some(new);
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
        self.save || self.save_as || self.undo || self.redo
            || self.reset_all || self.rot_cw || self.rot_ccw
            || self.flip_h    || self.flip_v
    }
    fn has_any(&self) -> bool {
        self.has_explicit_edit() || self.new_params.is_some()
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn update_title(ctx: &egui::Context, name: Option<&str>) {
    let title = name.map(|n| format!("{APP_NAME} — {n}"))
        .unwrap_or_else(|| APP_NAME.to_string());
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
}

/// Clamp `w×h` to 90% of the screen rect, with a minimum of `MIN_WINDOW`.
fn clamp_to_work_area(w: u32, h: u32, ctx: &egui::Context) -> (u32, u32) {
    let screen = ctx.screen_rect();
    let max_w  = (screen.width()  * 0.90) as u32;
    let max_h  = (screen.height() * 0.90) as u32;
    (w.min(max_w).max(MIN_WINDOW.0 as u32), h.min(max_h).max(MIN_WINDOW.1 as u32))
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

fn unique_rename_target(path: &Path, new_ext: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default();
    let dir  = path.parent().unwrap_or(Path::new("."));
    let base = dir.join(format!("{}.{}", stem.to_string_lossy(), new_ext));
    if !base.exists() { return base; }
    for i in 2u32.. {
        let candidate = dir.join(format!("{}_{}.{}", stem.to_string_lossy(), i, new_ext));
        if !candidate.exists() { return candidate; }
    }
    base // unreachable in practice
}

/// Floating-point equality test used for change detection in `EditParams`.
/// Discrete fields (rotation, flip, preset) use `==`; continuous fields use ε.
fn params_equal(a: &EditParams, b: &EditParams) -> bool {
    a.rotation == b.rotation && a.flip_h == b.flip_h && a.flip_v == b.flip_v &&
    a.preset   == b.preset   &&
    close(a.scale,      b.scale)      && close(a.exposure,   b.exposure)   &&
    close(a.highlights, b.highlights) && close(a.shadows,    b.shadows)    &&
    close(a.brightness, b.brightness) && close(a.contrast,   b.contrast)   &&
    close(a.saturation, b.saturation) && close(a.vibrance,   b.vibrance)   &&
    close(a.sharpen,    b.sharpen)
}

#[inline] fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }

// ── Lossless JPEG geometry transforms ─────────────────────────────────────────

/// Returns `true` when params contain only rotation/flip — no tonal, colour,
/// scale, or preset changes. These can be applied losslessly to JPEG files.
fn is_pure_geometry(p: &EditParams) -> bool {
    p.preset    == editor::Preset::None  &&
    (p.scale   - 1.0).abs() < 1e-4      &&
    p.exposure.abs()   < 1e-4 && p.highlights.abs() < 1e-4 &&
    p.shadows.abs()    < 1e-4 && p.brightness.abs() < 1e-4 &&
    p.contrast.abs()   < 1e-4 && p.saturation.abs() < 1e-4 &&
    p.vibrance.abs()   < 1e-4 && p.sharpen.abs()    < 1e-4
}

fn is_jpeg_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("jpg") | Some("jpeg")
    )
}

/// Apply rotation and/or flip to a JPEG file in-place without re-encoding.
///
/// Operates on DCT coefficients — no pixel decode/recode, no quality loss.
/// Returns `Ok(true)` on success, `Ok(false)` if the image dimensions are not
/// MCU-aligned (mozjpeg limitation — caller should fall back to re-encode),
/// or `Err` on I/O or parse failure.
/// Rotate/flip a JPEG file using mozjpeg for maximum quality preservation.
///
/// The term "lossless" here means: we re-encode at the SAME quality level as
/// the original file (read via mozjpeg's Decompress), apply the geometry
/// transform at the pixel level, then encode at that recovered quality.
/// This is the best possible quality-preserving approach available through the
/// mozjpeg 0.10 safe API. True DCT-coefficient lossless requires mozjpeg-sys
/// raw bindings and is a future enhancement.
///
/// Returns `Ok(true)` on success, `Ok(false)` for identity (nothing to do),
/// or `Err` on I/O / encode failure.
fn lossless_jpeg_transform(path: &Path, p: &EditParams) -> anyhow::Result<bool> {
    // Identity check — nothing to do.
    if p.rotation == editor::Rotation::R0 && !p.flip_h && !p.flip_v {
        return Ok(false);
    }

    let jpeg_bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("could not read {}: {e}", path.display()))?;

    // Decode JPEG → RGB pixels.
    let (pixels, src_w, src_h) = {
        let decomp = mozjpeg::Decompress::new_mem(&jpeg_bytes)
            .map_err(|e| anyhow::anyhow!("JPEG decode failed: {e}"))?;
        let mut img = decomp.rgb()
            .map_err(|e| anyhow::anyhow!("JPEG colorspace failed: {e}"))?;
        let w = img.width()  as u32;
        let h = img.height() as u32;
        // read_scanlines::<u8>() returns a flat Vec<u8> of RGB bytes
        let px: Vec<u8> = img.read_scanlines::<u8>()
            .map_err(|e| anyhow::anyhow!("JPEG scanlines failed: {e}"))?;
        (px, w, h)
    };

    // Apply geometry transform at the pixel level.
    let (transformed, out_w, out_h) = transform_rgb_pixels(&pixels, src_w, src_h, p);

    // Re-encode at quality 95 — high quality while staying smaller than lossless PNG.
    // (True lossless JPEG via DCT coefficient transforms requires mozjpeg-sys raw bindings.)
    const ENCODE_QUALITY: f32 = 95.0;
    let encoded = {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        comp.set_size(out_w as usize, out_h as usize);
        comp.set_quality(ENCODE_QUALITY);
        comp.set_optimize_coding(true);
        let mut started = comp.start_compress(Vec::new())
            .map_err(|e| anyhow::anyhow!("JPEG compress start failed: {e}"))?;
        started.write_scanlines(&transformed)
            .map_err(|e| anyhow::anyhow!("JPEG write scanlines failed: {e}"))?;
        started.finish()
            .map_err(|e| anyhow::anyhow!("JPEG finish failed: {e}"))?
    };

    std::fs::write(path, &encoded)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", path.display()))?;
    log!("[save] JPEG geometry saved at quality={ENCODE_QUALITY}");
    Ok(true)
}

/// Apply rotation and flip to a flat RGB (3 bytes/pixel) pixel buffer.
/// Returns (transformed_rgb_pixels, new_width, new_height).
fn transform_rgb_pixels(
    px: &[u8], w: u32, h: u32, p: &EditParams,
) -> (Vec<u8>, u32, u32) {
    // Widen RGB → RGBA so our existing geometry pipeline can work on it.
    let rgba: Vec<u8> = px.chunks_exact(3)
        .flat_map(|c| [c[0], c[1], c[2], 255u8])
        .collect();

    let (rotated, out_w, out_h) = editor::apply_rotation_flip_raw(&rgba, w, h, p);

    // Narrow RGBA → RGB for the mozjpeg encoder.
    let rgb: Vec<u8> = rotated.chunks_exact(4)
        .flat_map(|c| [c[0], c[1], c[2]])
        .collect();
    (rgb, out_w, out_h)
}

fn save_image_to_path(img: &DecodedImage, path: &Path, frame: usize) -> anyhow::Result<()> {
    use image::{ImageBuffer, Rgba, DynamicImage};
    let frame  = img.frame(frame);
    let rgba: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(img.width, img.height, frame.pixels.clone())
            .ok_or_else(|| anyhow::anyhow!("pixel buffer size mismatch: {}×{}", img.width, img.height))?;
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    // JPEG and BMP have no alpha channel — convert to RGB before encoding.
    if matches!(ext.as_deref(), Some("jpg") | Some("jpeg") | Some("bmp")) {
        DynamicImage::ImageRgba8(rgba).to_rgb8().save(path)?;
    } else {
        rgba.save(path)?;
    }
    Ok(())
}

fn install_fonts(ctx: &egui::Context) {
    // To bundle Inter: place InterVariable.ttf at src/assets/fonts/ and uncomment:
    // let mut fonts = egui::FontDefinitions::default();
    // fonts.font_data.insert("Inter".to_owned(),
    //     egui::FontData::from_static(include_bytes!("assets/fonts/InterVariable.ttf")));
    // fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "Inter".to_owned());
    // ctx.set_fonts(fonts);
    let _ = ctx;
}

fn load_icon() -> egui::IconData {
    // Place a 256×256 RGBA PNG at src/assets/icon.png and use:
    //   egui::IconData::try_from_png_bytes(include_bytes!("assets/icon.png")).unwrap_or_default()
    egui::IconData::default()
}

fn image_size_hint(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path).ok()?
        .with_guessed_format().ok()?
        .into_dimensions().ok()
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    init_log();

    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);
    let (init_w, init_h) = initial_path.as_deref()
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
                LoadOutcome::Ok(img)                  => Msg::Loaded { path, image: img, mode: OpenMode::FirstImage },
                LoadOutcome::WrongExtension(img, ext) => Msg::WrongExtension { path, image: img, mode: OpenMode::FirstImage, correct_ext: ext },
                LoadOutcome::Unsupported(reason)      => Msg::Unsupported { path, reason },
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
        renderer:     eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode:     eframe::wgpu::PresentMode::AutoVsync,
            power_preference: eframe::wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(APP_NAME, options, Box::new(move |cc| {
        install_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        Box::new(app) as Box<dyn eframe::App>
    })).expect("eframe failed to start");
}
