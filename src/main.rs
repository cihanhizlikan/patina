//! main.rs — eframe/egui rendering, cross-platform.
//!
//! Architecture
//! ============
//! - eframe manages the window, event loop, and wgpu/egui backend.
//! - Image pixels → wgpu texture → registered with egui via register_native_texture
//!   → displayed as egui::Image in the CentralPanel. Zero pixel copies.
//! - Editor panel → egui SidePanel (unchanged from previous architecture).
//! - All app logic (loading, navigation, editing) is identical to before.
//!
//! Cross-platform notes
//! ====================
//! - No #[cfg(windows)] blocks in this file. The Win32 SC_MAXIMIZE/SC_CLOSE
//!   hooks are removed; eframe handles window management natively per platform.
//! - rfd (Save As dialog) is already cross-platform.
//! - build.rs / winres still embeds the Windows icon but is a no-op on Linux/macOS.

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

// ── Type aliases ──────────────────────────────────────────────────────────────

type DirWatcher = Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>;

// ── Constants ─────────────────────────────────────────────────────────────────

const APP_NAME:           &str     = "Patina";
const DIR_WATCH_DEBOUNCE: Duration = Duration::from_millis(50);
/// Canvas background — matches ui::CANVAS (#0e0e10).
const CANVAS_COLOR: egui::Color32 = egui::Color32::from_rgb(0x0E, 0x0E, 0x10);

// ── Messages ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Msg {
    Loaded         { path: PathBuf, image: DecodedImage, mode: OpenMode },
    WrongExtension { path: PathBuf, image: DecodedImage, mode: OpenMode, correct_ext: String },
    Unsupported    { path: PathBuf, reason: String },
    Preloaded      { path: PathBuf, image: DecodedImage },
    Deleted        { path: PathBuf },
    DirectoryChanged,
}

#[derive(Default)]
struct DrainResult {
    resize_to:    Option<(u32, u32)>,
    error_msg:    Option<String>,
    rename_offer: Option<(PathBuf, String)>,
}

// ── Modal dialog state ────────────────────────────────────────────────────────

#[derive(Debug)]
enum ModalDialog {
    SaveOnExit    { save_and_quit: bool },
    ConfirmDelete { filename: String },
    OfferRename   { old_path: PathBuf, new_path: PathBuf, old_ext: String, new_ext: String },
    Error         { message: String },
}

// ── Image texture handle ──────────────────────────────────────────────────────

/// Holds a wgpu texture registered with egui via `register_native_texture`.
/// The egui TextureId is stable across frames as long as we don't re-register.
struct ImageTexture {
    texture:    wgpu::Texture,
    texture_id: egui::TextureId,
    width:      u32,
    height:     u32,
}

// ── Save As modal state ──────────────────────────────────────────────────────

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
    directory: PathBuf,  // default save location
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

struct App {
    // Image display
    current_path:          Option<PathBuf>,
    current_image:         Option<DecodedImage>,
    frame_index:           usize,
    next_frame_at:         Option<Instant>,
    /// Native wgpu texture registered with egui for zero-copy display.
    image_texture:         Option<ImageTexture>,
    last_uploaded_gen:     u64,
    last_uploaded_frame:   usize,

    // Navigation & loading
    navigator:             DirNavigator,
    msg_tx:                Sender<Msg>,
    msg_rx:                Receiver<Msg>,
    loading_path:          Option<PathBuf>,
    preload_cache:         HashMap<PathBuf, DecodedImage>,
    deleting:              bool,
    dir_watcher:           Option<DirWatcher>,
    watched_dir:           Option<PathBuf>,

    // Window state
    fullscreen:            bool,
    maximized:             bool,
    pre_maximize_size:     Option<egui::Vec2>,
    image_gen:             u64,

    // Editor
    editor_state:          Option<EditorState>,
    initial_editor_params: Option<EditParams>,
    editor_open_sections:  [bool; 5],
    thumb_cache:           Option<ThumbCache>,
    icon_cache:            Option<Icons>,

    // Async editing
    edit_rx:               Option<crossbeam_channel::Receiver<(u64, DecodedImage)>>,
    edit_gen:              u64,
    last_slider_change:    Option<Instant>,

    // Modal & deferred actions
    modal:                    Option<ModalDialog>,
    save_as_modal:            Option<SaveAsModal>,
    pending_error:            Option<String>,
    pending_rename:           Option<(PathBuf, String)>,
    pending_confirmed_delete: bool,
    pending_save_and_exit:    Option<bool>,
}

impl App {
    fn new(msg_tx: Sender<Msg>, msg_rx: Receiver<Msg>) -> Self {
        App {
            current_path:          None,
            current_image:         None,
            frame_index:           0,
            next_frame_at:         None,
            image_texture:         None,
            last_uploaded_gen:     u64::MAX,
            last_uploaded_frame:   usize::MAX,
            navigator:             DirNavigator::default(),
            msg_tx,
            msg_rx,
            loading_path:          None,
            preload_cache:         HashMap::new(),
            deleting:              false,
            dir_watcher:           None,
            watched_dir:           None,
            fullscreen:            false,
            maximized:             false,
            pre_maximize_size:     None,
            image_gen:             0,
            editor_state:          None,
            initial_editor_params: None,
            editor_open_sections:  [true, true, false, false, false],
            thumb_cache:           None,
            icon_cache:            None,
            edit_rx:               None,
            edit_gen:              0,
            last_slider_change:    None,
            modal:                 None,
            save_as_modal:         None,
            pending_error:         None,
            pending_rename:        None,
            pending_confirmed_delete: false,
            pending_save_and_exit:    None,
        }
    }

    fn in_editor(&self) -> bool { self.editor_state.is_some() }

    fn view(&self) -> Option<&DecodedImage> {
        if let Some(es) = &self.editor_state { Some(&es.display) }
        else { self.current_image.as_ref() }
    }

    fn current_filename(&self) -> Option<String> {
        self.current_path.as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    }

    // ── Image loading ─────────────────────────────────────────────────────────

    fn open_image(&mut self, path: PathBuf, mode: OpenMode, ctx: &egui::Context) {
        if let Some(cached) = self.preload_cache.remove(&path) {
            self.apply_image(path, cached, mode, ctx);
            ctx.request_repaint();
            return;
        }
        if self.loading_path.as_deref() == Some(&path) { return; }
        self.loading_path = Some(path.clone());
        let tx  = self.msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match load_image(&path) {
                LoadOutcome::Ok(img)                   => Msg::Loaded { path, image: img, mode },
                LoadOutcome::WrongExtension(img, ext)  => Msg::WrongExtension { path, image: img, mode, correct_ext: ext },
                LoadOutcome::Unsupported(reason)       => Msg::Unsupported { path, reason },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }



    /// Toggle managed-maximise: save current size, fill the work area; or restore.
    fn toggle_maximize(&mut self, ctx: &egui::Context) {
        if self.maximized {
            if let Some(saved) = self.pre_maximize_size.take() {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(saved));
                if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) { ctx.send_viewport_cmd(cmd); }
            }
            self.maximized = false;
        } else {
            let current = ctx.screen_rect().size();
            self.pre_maximize_size = Some(current);
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            self.maximized = true;
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    // ── Editor lifecycle ──────────────────────────────────────────────────────

    fn enter_editor(&mut self) {
        let Some(img) = &self.current_image else { return };
        if self.editor_state.is_some() { return; }
        let params = EditParams::default();
        let es     = EditorState::new(&img);
        self.initial_editor_params = Some(params);
        self.editor_state          = Some(es);
        self.thumb_cache           = Some(ThumbCache::new());
    }

    fn exit_editor(&mut self, quit_intent: bool) -> bool {
        if self.editor_state.is_none() { return true; }
        let changed = self.editor_params_changed();
        if changed && self.modal.is_none() {
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
    }

    fn editor_params_changed(&self) -> bool {
        match (&self.editor_state, &self.initial_editor_params) {
            (Some(es), Some(init)) => !params_equal(es.undo.current(), init),
            _ => false,
        }
    }

    fn editor_params_mut(&mut self, f: impl FnOnce(&mut EditParams)) -> EditParams {
        if let Some(es) = &mut self.editor_state {
            let mut p = es.undo.current().clone();
            f(&mut p);
            p
        } else { EditParams::default() }
    }

    fn editor_undo(&mut self) { self.editor_step(|u| u.undo()); }
    fn editor_redo(&mut self) { self.editor_step(|u| u.redo()); }

    fn editor_step(&mut self, f: impl FnOnce(&mut editor::UndoStack)) {
        if let Some(es) = &mut self.editor_state { f(&mut es.undo); es.dirty = true; }
        if let Some(p) = self.editor_state.as_ref().map(|es| es.undo.current().clone()) {
            self.launch_async_edit(p);
        }
    }

    fn editor_commit_and_sync(&mut self, p: EditParams) {
        self.last_slider_change = None;
        if let Some(es) = &mut self.editor_state { es.commit(p.clone()); }
        self.launch_async_edit(p);
    }

    fn launch_async_edit(&mut self, params: EditParams) {
        let Some(orig) = self.current_image.clone() else { return };
        self.edit_gen += 1;
        let gen      = self.edit_gen;
        let (tx, rx) = bounded(1);
        self.edit_rx = Some(rx);
        let p        = params;
        std::thread::spawn(move || {
            let result = apply_edits(&orig, &p);
            let _ = tx.send((gen, result));
        });
    }

    fn poll_edit_result(&mut self) -> bool {
        let Some(rx) = &self.edit_rx else { return false };
        match rx.try_recv() {
            Ok((gen, img)) => {
                self.edit_rx = None;
                if gen == self.edit_gen {
                    if let Some(es) = &mut self.editor_state {
                        es.display  = img;
                        es.dirty    = false;
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

    // ── Navigation ────────────────────────────────────────────────────────────

    fn navigate(&mut self, forward: bool, ctx: &egui::Context) {
        let next = if forward { self.navigator.next() } else { self.navigator.prev() };
        if let Some(p) = next { self.open_image(p, OpenMode::Navigate, ctx); }
    }

    fn clear_image(&mut self) {
        self.current_image  = None;
        self.current_path   = None;
        self.frame_index    = 0;
        self.next_frame_at  = None;
        self.image_gen     += 1;
    }

    fn apply_image(&mut self, path: PathBuf, img: DecodedImage, mode: OpenMode, ctx: &egui::Context) {
        self.loading_path = None;
        self.deleting     = false;
        self.frame_index  = 0;

        // Schedule first animation frame if GIF
        if img.frames.len() > 1 {
            self.next_frame_at = Some(Instant::now() + Duration::from_millis(img.frame(0).delay_ms as u64));
        } else {
            self.next_frame_at = None;
        }

        let is_new_dir = self.current_path.as_deref()
            .and_then(|p| p.parent())
            != path.parent();

        self.current_image = Some(img.clone());
        self.current_path  = Some(path.clone());
        self.image_gen    += 1;

        if is_new_dir || mode == OpenMode::FirstImage {
            self.navigator.set_current(path.clone());
            self.watch_directory(path.parent().map(|p| p.to_path_buf()), ctx);
        }

        // Preload neighbours
        self.preload_neighbour(true,  &img, ctx);
        self.preload_neighbour(false, &img, ctx);
    }

    fn preload_neighbour(&mut self, forward: bool, _current: &DecodedImage, ctx: &egui::Context) {
        let target = if forward { self.navigator.next() } else { self.navigator.prev() };
        if let Some(p) = target {
            if !self.preload_cache.contains_key(&p) && self.loading_path.as_deref() != Some(&p) {
                let tx  = self.msg_tx.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    if let LoadOutcome::Ok(img) = load_image(&p) {
                        let _ = tx.send(Msg::Preloaded { path: p, image: img });
                        ctx.request_repaint();
                    }
                });
            }
        }
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

    fn tick_animation(&mut self) -> bool {
        let Some(at) = self.next_frame_at else { return false };
        if Instant::now() < at { return false; }
        if let Some(img) = &self.current_image {
            let n = img.frames.len();
            if n > 1 {
                self.frame_index = (self.frame_index + 1) % n;
                let delay = img.frame(self.frame_index).delay_ms;
                self.next_frame_at = Some(Instant::now() + Duration::from_millis(delay as u64));
                self.image_gen += 1;
                return true;
            }
        }
        self.next_frame_at = None;
        false
    }

    fn drain_messages(&mut self, ctx: &egui::Context) -> DrainResult {
        let mut dr = DrainResult::default();
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::Loaded { path, image, mode } => {
                    let (w, h) = (image.width, image.height);
                    self.apply_image(path, image, mode, ctx);
                    dr.resize_to = Some((w, h));
                }
                Msg::WrongExtension { path, image, mode, correct_ext } => {
                    let (w, h) = (image.width, image.height);
                    self.apply_image(path.clone(), image, mode, ctx);
                    dr.resize_to    = Some((w, h));
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
                Msg::DirectoryChanged => {
                    if let Some(dir) = &self.watched_dir {
                        self.navigator.rebuild_current(Some(dir.as_path()));
                    }
                }
            }
        }
        dr
    }

    // ── Editor actions ────────────────────────────────────────────────────────

    fn apply_panel_action(&mut self, action: PanelAction, ctx: &egui::Context) {
        // Clear debounce on any explicit action
        if action.save || action.save_as || action.undo || action.redo || action.reset_all
            || action.rot_cw || action.rot_ccw || action.flip_h || action.flip_v
        { self.last_slider_change = None; }

        if action.save    { log!("[action] Save");    self.save_inplace(ctx); return; }
        if action.save_as { log!("[action] SaveAs");  self.save_as_returning(ctx); return; }
        if action.undo    { log!("[action] Undo");    self.editor_undo(); return; }
        if action.redo    { log!("[action] Redo");    self.editor_redo(); return; }

        if action.reset_all { log!("[action] ResetAll");
            self.editor_commit_and_sync(EditParams::default()); return;
        }
        if action.rot_cw  { log!("[action] RotCw");
            let p = self.editor_params_mut(|p| p.rotation = p.rotation.cw());
            self.editor_commit_and_sync(p); return;
        }
        if action.rot_ccw { log!("[action] RotCcw");
            let p = self.editor_params_mut(|p| p.rotation = p.rotation.ccw());
            self.editor_commit_and_sync(p); return;
        }
        if action.flip_h  { log!("[action] FlipH");
            let p = self.editor_params_mut(|p| p.flip_h = !p.flip_h);
            self.editor_commit_and_sync(p); return;
        }
        if action.flip_v  { log!("[action] FlipV");
            let p = self.editor_params_mut(|p| p.flip_v = !p.flip_v);
            self.editor_commit_and_sync(p); return;
        }

        if let Some(new_p) = action.new_params {
            log!("[action] ParamChange");
            if let Some(es) = &mut self.editor_state { es.commit(new_p.clone()); }
            self.last_slider_change = Some(Instant::now());
        }
    }

    // ── Save / delete ─────────────────────────────────────────────────────────

    fn save_inplace(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current_path.clone() else { return };
        let Some(es)   = &self.editor_state         else { return };
        let params     = es.undo.current().clone();

        // For JPEG saves, attempt lossless transform when only rotation/flip
        // changed (no pixel-level edits). Falls back to re-encode on failure.
        let is_jpeg = matches!(
            path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("jpg") | Some("jpeg")
        );
        let pure_geometry = is_jpeg
            && params.exposure.abs()   < 1e-4
            && params.highlights.abs() < 1e-4
            && params.shadows.abs()    < 1e-4
            && params.brightness.abs() < 1e-4
            && params.contrast.abs()   < 1e-4
            && params.saturation.abs() < 1e-4
            && params.vibrance.abs()   < 1e-4
            && params.sharpen.abs()    < 1e-4
            && params.preset == editor::Preset::None
            && (params.scale - 1.0).abs() < 1e-4;

        // TODO agenda item 3: attempt lossless JPEG transform here when pure_geometry
        //   is true and mozjpeg is available — will avoid re-encoding.
        let _ = pure_geometry;

        if let Err(e) = save_image_to_path(&es.display, &path, self.frame_index) {
            self.pending_error = Some(format!("Save failed:\n{e}"));
        } else {
            update_title(ctx, self.current_filename().as_deref());
            self.reset_editor_after_save();
        }
    }

    /// Open the custom Save As modal — actual write happens on confirm.
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

    /// Execute the actual write after Save As modal is confirmed.
    fn commit_save_as(&mut self, ctx: &egui::Context, dest: PathBuf) {
        let Some(es) = &self.editor_state else { return };
        let display_clone = es.display.clone();
        if let Err(e) = save_image_to_path(&display_clone, &dest, self.frame_index) {
            self.pending_error = Some(format!("Save failed:\n{e}"));
            return;
        }
        self.current_path  = Some(dest.clone());
        self.current_image = Some(display_clone);
        self.navigator.set_current(dest.clone());
        update_title(ctx, self.current_filename().as_deref());
        self.image_gen += 1;
        self.reset_editor_after_save();
    }

    fn reset_editor_after_save(&mut self) {
        if let Some(es) = &mut self.editor_state {
            let fresh = EditParams::default();
            es.undo.push(fresh);
            es.dirty = false;
        }
        self.initial_editor_params = Some(EditParams::default());
    }

    fn delete_current(&mut self) {
        let Some(path) = self.current_path.clone() else { return };
        if self.deleting { return; }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("this file");
        self.modal = Some(ModalDialog::ConfirmDelete { filename: fname.to_string() });
    }

    fn execute_delete(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current_path.clone() else { return };
        if self.deleting { return; }
        self.deleting = true;

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

// ── eframe::App implementation ────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // ── Drain background messages ──────────────────────────────────────
        let dr = self.drain_messages(ctx);
        if let Some((w, h)) = dr.resize_to {
            let (cw, ch) = clamp_to_work_area(w, h, ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(cw as f32, ch as f32)));
            if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) { ctx.send_viewport_cmd(cmd); }
        }
        if let Some(msg) = dr.error_msg {
            self.pending_error = Some(msg);
        }
        if let Some((old_path, ext)) = dr.rename_offer {
            self.pending_rename = Some((old_path, ext));
        }

        // ── Deferred modal actions ─────────────────────────────────────────
        if let Some(quit_after) = self.pending_save_and_exit.take() {
            self.save_inplace(ctx);
            self.do_exit_editor();
            if quit_after { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }
        if self.pending_confirmed_delete {
            self.pending_confirmed_delete = false;
            self.execute_delete(ctx);
        }

        // ── Animation tick ────────────────────────────────────────────────
        if self.tick_animation() { ctx.request_repaint(); }

        // ── Debounced slider reprocess ─────────────────────────────────────
        if self.last_slider_change.is_some_and(|t| t.elapsed().as_millis() >= 80) {
            self.last_slider_change = None;
            if self.in_editor() {
                if let Some(es) = &self.editor_state {
                    log!("[debounce] firing async edit after slider idle");
                    let p = es.undo.current().clone();
                    self.launch_async_edit(p);
                }
            }
        } else if self.last_slider_change.is_some() {
            ctx.request_repaint();
        }

        // ── Poll async edit result ─────────────────────────────────────────
        if self.poll_edit_result() {
            ctx.request_repaint();
        } else if self.edit_rx.is_some() {
            ctx.request_repaint();
        }

        // ── Upload image to GPU texture ────────────────────────────────────
        if self.image_gen != self.last_uploaded_gen || self.frame_index != self.last_uploaded_frame {
            self.last_uploaded_gen   = self.image_gen;
            self.last_uploaded_frame = self.frame_index;
            // Extract pixel data before taking the mutable borrow on image_texture.
            // self.view() borrows self immutably; we clone out what we need first.
            let upload_data: Option<(Vec<u8>, u32, u32)> = self.view().map(|img| {
                let frame_data = img.frame(self.frame_index);
                (frame_data.pixels.clone(), img.width, img.height)
            });
            if let Some((pixels, w, h)) = upload_data {
                let t_up = Instant::now();
                upload_image_texture(ctx, frame, &pixels, w, h, &mut self.image_texture);
                let up_ms = t_up.elapsed().as_millis();
                if up_ms > 2 { log!("[upload] {}x{} {}ms", w, h, up_ms); }
            }
        }

        // ── Apply global UI style ──────────────────────────────────────────
        ui::apply_panel_style(ctx);

        // ── Pending error → modal ──────────────────────────────────────────
        if let Some(msg) = self.pending_error.take() {
            self.modal = Some(ModalDialog::Error { message: msg });
        }
        // ── Pending rename offer → modal ───────────────────────────────────
        if let Some((old_path, correct_ext)) = self.pending_rename.take() {
            if let Some(new_path) = offer_rename_if_writable(&old_path, &correct_ext) {
                let old_ext = old_path.extension()
                    .and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                self.modal = Some(ModalDialog::OfferRename {
                    old_path, new_path, old_ext, new_ext: correct_ext,
                });
            }
        }

        // ── Editor icon/thumb cache initialisation ─────────────────────────
        if self.in_editor() {
            if let Some(tc) = &mut self.thumb_cache { tc.upload(ctx); }
            if self.icon_cache.is_none() { self.icon_cache = Some(Icons::load(ctx)); }
        }

        // ── Modal dialogs (always on top) ──────────────────────────────────
        let icons_ptr: Option<*const ui::Icons> =
            self.icon_cache.as_ref().map(|ic| ic as *const ui::Icons);
        let icons_ref: Option<&ui::Icons> =
            // SAFETY: icon_cache is never modified inside draw_modal.
            icons_ptr.map(|p| unsafe { &*p });
        draw_modal(ctx, self, icons_ref);
        draw_save_as_modal(ctx, self);

        // ── Editor panel (SidePanel — MUST precede CentralPanel) ───────────
        let mut pending_action: Option<PanelAction> = None;
        if self.in_editor() {
            let params   = self.editor_state.as_ref().map(|es| es.undo.current().clone()).unwrap_or_default();
            let can_undo = self.editor_state.as_ref().map(|es| es.undo.can_undo()).unwrap_or(false);
            let can_redo = self.editor_state.as_ref().map(|es| es.undo.can_redo()).unwrap_or(false);
            let action   = draw_panel(
                ctx, &params, can_undo, can_redo,
                self.thumb_cache.as_ref().unwrap(),
                self.icon_cache.as_ref().unwrap(),
                &mut self.editor_open_sections,
            );
            if action.save || action.save_as || action.undo || action.redo
                || action.reset_all || action.rot_cw || action.rot_ccw
                || action.flip_h || action.flip_v || action.new_params.is_some()
            {
                pending_action = Some(action);
            }
        }

        // ── Central panel — canvas area ────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(CANVAS_COLOR))
            .show(ctx, |ui| {
                if let Some(it) = &self.image_texture {
                    let canvas = ui.available_rect_before_wrap();
                    let scale  = self.editor_state.as_ref()
                        .map(|es| es.undo.current().scale)
                        .unwrap_or(1.0);

                    // Fit image to canvas, then apply user scale factor.
                    let fit_scale = (canvas.width()  / it.width  as f32)
                        .min(canvas.height() / it.height as f32);
                    let display_w = it.width  as f32 * fit_scale * scale;
                    let display_h = it.height as f32 * fit_scale * scale;

                    // Centre within the canvas.
                    let x = canvas.min.x + (canvas.width()  - display_w) * 0.5;
                    let y = canvas.min.y + (canvas.height() - display_h) * 0.5;
                    let img_rect = egui::Rect::from_min_size(
                        egui::pos2(x, y), egui::vec2(display_w, display_h));

                    egui::Image::new(egui::load::SizedTexture::new(
                        it.texture_id,
                        egui::vec2(it.width as f32, it.height as f32),
                    ))
                    .fit_to_exact_size(egui::vec2(display_w, display_h))
                    .paint_at(ui, img_rect);

                    // Double-click on canvas → toggle maximize.
                    let resp = ui.allocate_rect(img_rect, egui::Sense::click());
                    if resp.double_clicked() {
                        self.toggle_maximize(ctx);
                    }
                }

                // Mouse wheel → navigate in viewer mode.
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 && !self.in_editor() {
                    self.navigate(scroll < 0.0, ctx);
                }

                // Drag-and-drop file onto canvas.
                let dropped = ui.input(|i| {
                    i.raw.dropped_files.first()
                        .and_then(|f| f.path.clone())
                });
                if let Some(path) = dropped {
                    if !self.in_editor() {
                        self.open_image(path, OpenMode::Navigate, ctx);
                    }
                }
            });

        // ── Apply panel action after egui frame ────────────────────────────
        if let Some(action) = pending_action {
            self.apply_panel_action(action, ctx);
            ctx.request_repaint();
        }

        // ── Keyboard shortcuts ─────────────────────────────────────────────
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    handle_key(self, *key, modifiers, ctx);
                }
            }
        });

        // ── Schedule next animation frame ──────────────────────────────────
        if let Some(at) = self.next_frame_at {
            let remaining = at.saturating_duration_since(Instant::now());
            ctx.request_repaint_after(remaining);
        }
    }

    fn on_exit(&mut self) {
        // Nothing to clean up — threads will exit naturally.
    }
}

// ── GPU texture management ────────────────────────────────────────────────────

/// Upload pixel data to a wgpu texture and register it with egui via
/// `register_native_texture`. Reuses the existing texture when dimensions
/// match (in-place update). Creates a new texture when dimensions change.
///
/// Using `register_native_texture` means egui never touches the pixel data —
/// the TextureId is just a stable handle into egui's texture registry.
fn upload_image_texture(
    _ctx:    &egui::Context,
    frame:   &mut eframe::Frame,
    pixels:  &[u8],
    w:       u32,
    h:       u32,
    slot:    &mut Option<ImageTexture>,
) {
    if w == 0 || h == 0 { return; }

    let wgpu_render_state = match frame.wgpu_render_state() {
        Some(rs) => rs,
        None     => { eprintln!("[upload] wgpu render state unavailable"); return; }
    };
    let device = &wgpu_render_state.device;
    let queue  = &wgpu_render_state.queue;

    let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let layout = wgpu::ImageDataLayout {
        offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h),
    };

    if let Some(it) = slot.as_ref() {
        if it.width == w && it.height == h {
            // Same dimensions — update in-place, keep the same TextureId.
            let copy = wgpu::ImageCopyTexture {
                texture: &it.texture, mip_level: 0,
                origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
            };
            queue.write_texture(copy, pixels, layout, extent);
            return;
        }
        // Dimensions changed — unregister old texture.
        wgpu_render_state.renderer.write().free_texture(&it.texture_id);
    }

    // Create a new texture.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label:           Some("image"),
        size:            extent,
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        format:          wgpu::TextureFormat::Rgba8UnormSrgb,
        usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats:    &[],
    });
    let copy = wgpu::ImageCopyTexture {
        texture: &texture, mip_level: 0,
        origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
    };
    queue.write_texture(copy, pixels, layout, extent);

    let view       = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let texture_id = wgpu_render_state.renderer.write()
        .register_native_texture(device, &view, wgpu::FilterMode::Linear);

    *slot = Some(ImageTexture { texture, texture_id, width: w, height: h });
}

// ── Draw modal (unchanged from old main.rs, proxy → ctx) ─────────────────────

/// Draw the custom Save As modal — filename field, format selector, directory browse.
fn draw_save_as_modal(ctx: &egui::Context, app: &mut App) {
    let Some(ref mut modal) = app.save_as_modal else { return };

    use ui::{ACCENT, ACCENT_BG, BG, BORDER, BTN, DIM, ON_PRIMARY, TEXT};

    // Dim overlay
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("saveas_dim")))
        .rect_filled(ctx.screen_rect(), 0.0, egui::Color32::from_black_alpha(160));

    let mut confirm = false;
    let mut cancel  = false;
    let mut browse  = false;

    egui::Window::new("save_as_modal")
        .title_bar(false).resizable(false).collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::none()
            .fill(BG)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(24.0))
        .show(ctx, |ui| {
            ui.set_width(360.0);

            // Title
            ui.label(egui::RichText::new("Save As").color(TEXT).size(16.0).strong());
            ui.add_space(16.0);

            // Directory row
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Folder").color(DIM).size(12.0));
                ui.add_space(4.0);
                let dir_str = modal.directory.to_string_lossy();
                let display = if dir_str.len() > 38 {
                    format!("…{}", &dir_str[dir_str.len()-38..])
                } else { dir_str.to_string() };
                ui.label(egui::RichText::new(display).color(TEXT).size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(
                        egui::RichText::new("Browse…").size(12.0)).fill(BTN)
                        .stroke(egui::Stroke::new(1.0, BORDER))).clicked()
                    { browse = true; }
                });
            });
            ui.add_space(12.0);

            // Filename field
            ui.label(egui::RichText::new("Filename").color(DIM).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut modal.stem)
                        .desired_width(260.0)
                        .font(egui::FontId::proportional(13.0))
                );
                // Press Enter to confirm
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                ui.label(egui::RichText::new(
                    format!(".{}", modal.format.extension())
                ).color(DIM).size(13.0));
            });
            ui.add_space(12.0);

            // Format selector
            ui.label(egui::RichText::new("Format").color(DIM).size(12.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for &fmt in OutputFormat::ALL {
                    let active = modal.format == fmt;
                    let (bg, fg, stroke_w) = if active {
                        (ACCENT_BG, ACCENT, 1.0f32)
                    } else { (BTN, TEXT, 1.0) };
                    let bdr = if active { ACCENT } else { BORDER };
                    let btn = egui::Button::new(
                        egui::RichText::new(fmt.label()).color(fg).size(12.0))
                        .fill(bg)
                        .stroke(egui::Stroke::new(stroke_w, bdr))
                        .rounding(egui::Rounding::same(4.0));
                    if ui.add(btn).clicked() { modal.format = fmt; }
                }
            });
            ui.add_space(16.0);

            // Preview of final path
            ui.label(egui::RichText::new(
                format!("→ {}", modal.full_path().display())
            ).color(DIM).size(11.0));
            ui.add_space(16.0);

            // Action buttons
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Save — primary
                let save_lbl = egui::RichText::new("Save").color(ON_PRIMARY).size(13.0);
                let save_btn = egui::Button::new(save_lbl).fill(ACCENT)
                    .stroke(egui::Stroke::NONE)
                    .rounding(egui::Rounding::same(5.0))
                    .min_size(egui::vec2(72.0, 32.0));
                if ui.add(save_btn).clicked() { confirm = true; }
                // Cancel — default
                let cancel_btn = egui::Button::new(
                    egui::RichText::new("Cancel").color(TEXT).size(13.0))
                    .fill(BTN)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .rounding(egui::Rounding::same(5.0))
                    .min_size(egui::vec2(72.0, 32.0));
                if ui.add(cancel_btn).clicked() { cancel = true; }
            });
        });

    // Handle browse — blocks briefly for the native directory picker
    if browse {
        let start_dir = app.save_as_modal.as_ref().map(|m| m.directory.clone());
        if let Some(dir) = {
            let mut d = rfd::FileDialog::new().set_title("Choose folder");
            if let Some(sd) = start_dir { d = d.set_directory(sd); }
            d.pick_folder()
        } {
            if let Some(ref mut modal) = app.save_as_modal {
                modal.directory = dir;
            }
        }
    }

    if cancel { app.save_as_modal = None; }

    if confirm {
        if let Some(modal) = app.save_as_modal.take() {
            let dest = modal.full_path();
            // Avoid clobbering without warning — unique_rename_target handles collisions
            // but for Save As the user has explicitly named the file so we overwrite.
            app.commit_save_as(ctx, dest);
        }
    }
}

fn draw_modal(ctx: &egui::Context, app: &mut App, icons: Option<&ui::Icons>) {
    let Some(ref modal) = app.modal else { return };

    use egui::{Rounding, Sense, Stroke, Vec2};
    use ui::{
        ACCENT, ACCENT_BG, ACCENT_BDR, ACCENT_DIM, BG, BORDER,
        BTN, DANGER, DANGER_BG, DANGER_BDR, DANGER_PR, ON_PRIMARY, TEXT, DIM,
    };

    #[derive(Clone, Copy)]
    enum ModalBtnVariant { Primary, Default, Danger }

    fn modal_btn(
        ui: &mut egui::Ui, label: &str,
        icon: Option<&egui::TextureHandle>, variant: ModalBtnVariant,
    ) -> egui::Response {
        const PAD: f32 = 12.0; const GAP: f32 = 6.0;
        const ICON_SZ: f32 = 14.0; const FONT: f32 = 13.0;
        const HEIGHT: f32 = 32.0; const RADIUS: f32 = 5.0;

        let fg_measure = match variant { ModalBtnVariant::Primary => ON_PRIMARY, _ => TEXT };
        let ic_w  = if icon.is_some() { ICON_SZ + GAP } else { 0.0 };
        let lbl_w = ui.fonts(|f| f.layout_no_wrap(
            label.to_string(), egui::FontId::proportional(FONT), fg_measure).size().x);
        let w = PAD + ic_w + lbl_w + PAD;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, HEIGHT), Sense::click());
        let p    = ui.painter();
        let r    = Rounding::same(RADIUS);
        let down = resp.is_pointer_button_down_on();

        let (bg, fg, border) = match variant {
            ModalBtnVariant::Primary => {
                let bg = if down { ACCENT_DIM }
                    else if resp.hovered() { egui::Color32::from_rgb(0x8D, 0xAF, 0xA0) }
                    else { ACCENT };
                (bg, ON_PRIMARY, None)
            }
            ModalBtnVariant::Default => {
                let (bg, fg, bdr) = if down {
                    (egui::Color32::from_rgba_premultiplied(35,68,59,56), ACCENT, egui::Color32::from_rgba_premultiplied(71, 86, 79, 115))
                } else if resp.hovered() {
                    (ACCENT_BG, ACCENT, ACCENT_BDR)
                } else { (BTN, TEXT, BORDER) };
                (bg, fg, Some((1.0f32, bdr)))
            }
            ModalBtnVariant::Danger => {
                let (bg, fg, bdr) = if down {
                    (DANGER_PR, DANGER, DANGER_BDR)
                } else if resp.hovered() {
                    (DANGER_BG, DANGER, DANGER_BDR)
                } else { (BTN, TEXT, BORDER) };
                (bg, fg, Some((1.0f32, bdr)))
            }
        };

        p.rect_filled(rect, r, bg);
        if let Some((w, c)) = border { p.rect_stroke(rect, r, Stroke::new(w, c)); }
        let mut x = rect.min.x + PAD;
        if let Some(tex) = icon {
            let ir = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - ICON_SZ * 0.5), Vec2::splat(ICON_SZ));
            p.image(tex.id(), ir,
                egui::Rect::from_min_max(egui::pos2(0.,0.), egui::pos2(1.,1.)), fg);
            x += ICON_SZ + GAP;
        }
        p.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER,
            label, egui::FontId::proportional(FONT), fg);
        resp
    }

    let mbtn_primary = |ui: &mut egui::Ui, label, icon| modal_btn(ui, label, icon, ModalBtnVariant::Primary);
    let mbtn_default = |ui: &mut egui::Ui, label, icon| modal_btn(ui, label, icon, ModalBtnVariant::Default);
    let mbtn_danger  = |ui: &mut egui::Ui, label, icon| modal_btn(ui, label, icon, ModalBtnVariant::Danger);

    let screen = ctx.screen_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("modal_dim")))
        .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));

    let mut close  = false;
    let mut action: Option<&str> = None;

    egui::Window::new("modal")
        .title_bar(false).resizable(false).collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::none()
            .fill(BG).stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(10.0)).inner_margin(24.0))
        .show(ctx, |ui| {
            ui.set_width(340.0);
            match modal {
                ModalDialog::SaveOnExit { .. } => {
                    ui.label(egui::RichText::new("Save changes?").color(TEXT).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Your edits will be lost if you don't save.").color(DIM).size(13.0));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if mbtn_primary(ui, "Save",    icons.map(|ic| &ic.save)).clicked()    { action = Some("save");    close = true; }
                        if mbtn_default(ui, "Discard", None).clicked()                        { action = Some("discard"); close = true; }
                        if mbtn_default(ui, "Cancel",  None).clicked()                        { close = true; }
                    });
                }
                ModalDialog::ConfirmDelete { filename } => {
                    ui.label(egui::RichText::new("Delete image?").color(TEXT).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(
                        format!("Permanently delete\n\n{filename}?\n\nThis cannot be undone.")
                    ).color(DIM).size(13.0));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if mbtn_danger( ui, "Delete", icons.map(|ic| &ic.reset)).clicked()    { action = Some("delete"); close = true; }
                        if mbtn_default(ui, "Cancel", None).clicked()                         { close = true; }
                    });
                }
                ModalDialog::OfferRename { old_ext, new_ext, new_path, .. } => {
                    ui.label(egui::RichText::new("Wrong file extension").color(TEXT).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(
                        format!("This file is a .{new_ext} saved as .{old_ext}.\n\nRename to: {}",
                            new_path.file_name().unwrap_or_default().to_string_lossy())
                    ).color(DIM).size(13.0));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if mbtn_primary(ui, "Rename", icons.map(|ic| &ic.save)).clicked()     { action = Some("rename"); close = true; }
                        if mbtn_default(ui, "Keep",   None).clicked()                         { close = true; }
                    });
                }
                ModalDialog::Error { message } => {
                    ui.label(egui::RichText::new("Error").color(DANGER).size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(message.as_str()).color(TEXT).size(13.0));
                    ui.add_space(16.0);
                    if mbtn_default(ui, "OK", None).clicked() { close = true; }
                }
            }
        });

    if close {
        if let Some(a) = action { apply_modal_action(app, a); }
        app.modal = None;
    }
}

fn apply_modal_action(app: &mut App, action: &str) {
    match action {
        "save" => {
            if let Some(ref modal) = app.modal {
                if let ModalDialog::SaveOnExit { save_and_quit } = modal {
                    app.pending_save_and_exit = Some(*save_and_quit);
                }
            }
        }
        "discard" => { app.do_exit_editor(); }
        "delete"  => { app.pending_confirmed_delete = true; }
        "rename"  => {
            if let Some(ModalDialog::OfferRename { old_path, new_path, .. }) = &app.modal {
                let (old, new) = (old_path.clone(), new_path.clone());
                if std::fs::rename(&old, &new).is_ok() {
                    app.navigator.rename(&old, new.clone());
                    app.current_path = Some(new);
                }
            }
        }
        _ => {}
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn update_title(ctx: &egui::Context, name: Option<&str>) {
    let title = match name {
        Some(n) => format!("{APP_NAME} — {n}"),
        None    => APP_NAME.to_string(),
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
}

/// Clamp initial window size to 90% of the available work area.
fn clamp_to_work_area(w: u32, h: u32, ctx: &egui::Context) -> (u32, u32) {
    // eframe doesn't expose work area directly; use screen rect as approximation.
    let screen  = ctx.screen_rect();
    let max_w   = (screen.width()  * 0.90) as u32;
    let max_h   = (screen.height() * 0.90) as u32;
    (w.min(max_w).max(400), h.min(max_h).max(300))
}

fn offer_rename_if_writable(path: &Path, correct_ext: &str) -> Option<PathBuf> {
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
    base
}

fn params_equal(a: &EditParams, b: &EditParams) -> bool {
    a.rotation == b.rotation && a.flip_h == b.flip_h && a.flip_v == b.flip_v &&
    a.preset   == b.preset   &&
    (a.scale      - b.scale     ).abs() < 1e-4 &&
    (a.exposure   - b.exposure  ).abs() < 1e-4 &&
    (a.highlights - b.highlights).abs() < 1e-4 &&
    (a.shadows    - b.shadows   ).abs() < 1e-4 &&
    (a.brightness - b.brightness).abs() < 1e-4 &&
    (a.contrast   - b.contrast  ).abs() < 1e-4 &&
    (a.saturation - b.saturation).abs() < 1e-4 &&
    (a.vibrance   - b.vibrance  ).abs() < 1e-4 &&
    (a.sharpen    - b.sharpen   ).abs() < 1e-4
}

fn save_image_to_path(img: &DecodedImage, path: &Path, frame: usize) -> anyhow::Result<()> {
    use image::{ImageBuffer, Rgba, DynamicImage};
    let frame  = img.frame(frame);
    let pixels = &frame.pixels;
    let rgba: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(img.width, img.height, pixels.clone())
            .ok_or_else(|| anyhow::anyhow!("pixel buffer size mismatch"))?;
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    let needs_rgb = matches!(ext.as_deref(), Some("jpg") | Some("jpeg") | Some("bmp"));
    if needs_rgb {
        DynamicImage::ImageRgba8(rgba).to_rgb8().save(path)?;
    } else {
        rgba.save(path)?;
    }
    Ok(())
}

// Note: lossless JPEG rotate/flip via mozjpeg is agenda item 3.
// Stub removed — mozjpeg dependency not yet added to Cargo.toml.

fn install_fonts(ctx: &egui::Context) {
    // Inter font: place InterVariable.ttf at src/assets/fonts/ and uncomment:
    // let mut fonts = egui::FontDefinitions::default();
    // fonts.font_data.insert("Inter".to_owned(), egui::FontData::from_static(
    //     include_bytes!("assets/fonts/InterVariable.ttf")));
    // fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "Inter".to_owned());
    // ctx.set_fonts(fonts);
    let _ = ctx;
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    init_log();

    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    let (init_w, init_h) = initial_path.as_deref()
        .and_then(image_size_hint)
        .unwrap_or((900, 680));

    let (msg_tx, msg_rx) = bounded::<Msg>(32);
    let app = App::new(msg_tx, msg_rx);

    if let Some(path) = initial_path.clone() {
        // Pre-queue the load — it'll fire once the egui context is ready.
        let tx = app.msg_tx.clone();
        std::thread::spawn(move || {
            let msg = match load_image(&path) {
                LoadOutcome::Ok(img)                   => Msg::Loaded { path, image: img, mode: OpenMode::FirstImage },
                LoadOutcome::WrongExtension(img, ext)  => Msg::WrongExtension { path, image: img, mode: OpenMode::FirstImage, correct_ext: ext },
                LoadOutcome::Unsupported(reason)       => Msg::Unsupported { path, reason },
            };
            let _ = tx.send(msg);
        });
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size(egui::vec2(init_w as f32, init_h as f32))
            .with_min_inner_size(egui::vec2(400.0, 300.0))
            .with_app_id(APP_NAME)
            .with_icon(load_icon()),
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
        native_options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Box::new(app) as Box<dyn eframe::App>
        }),
    ).expect("eframe failed to start");
}

fn load_icon() -> egui::IconData {
    // Embed the application icon at compile time.
    // Place a 256×256 RGBA PNG at src/assets/icon.png to use it.
    // Falls back to an empty icon if the file is absent.
    egui::IconData::default()
}

fn image_size_hint(path: &Path) -> Option<(u32, u32)> {
    let r = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let (w, h) = r.into_dimensions().ok()?;
    Some((w, h))
}

/// egui keyboard key → app action.
fn handle_key(app: &mut App, key: egui::Key, mods: &egui::Modifiers, ctx: &egui::Context) {
    let ctrl  = mods.ctrl;
    let shift = mods.shift;

    if app.in_editor() {
        match (ctrl, shift, key) {
            (true,  false, egui::Key::Z)      => { app.editor_undo(); }
            (true,  true,  egui::Key::Z) |
            (true,  false, egui::Key::Y)      => { app.editor_redo(); }
            (true,  false, egui::Key::S)      => { app.save_inplace(ctx); }
            (true,  true,  egui::Key::S)      => { app.save_as_returning(ctx); }
            (false, false, egui::Key::E)  |
            (false, false, egui::Key::Escape) => { app.exit_editor(false); }
            (false, false, egui::Key::Q)      => {
                if app.exit_editor(true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            _ => {}
        }
        return;
    }

    match (ctrl, shift, key) {
        (false, false, egui::Key::ArrowRight) |
        (false, false, egui::Key::PageDown)   => { app.navigate(true,  ctx); }
        (false, false, egui::Key::ArrowLeft)  |
        (false, false, egui::Key::PageUp)     => { app.navigate(false, ctx); }
        (false, false, egui::Key::E)          => {
            app.enter_editor();
            if app.thumb_cache.is_none() { app.thumb_cache = Some(ThumbCache::new()); }
        }
        (false, false, egui::Key::F)          => { app.toggle_fullscreen(ctx); }
        (false, false, egui::Key::Escape)     => {
            if app.fullscreen { app.toggle_fullscreen(ctx); }
        }
        (false, false, egui::Key::Delete)     => { app.delete_current(); }
        (false, false, egui::Key::Q)          => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        _ => {}
    }
}
