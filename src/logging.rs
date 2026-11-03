//! logging.rs — Where log lines go.
//!
//! Patina logs through the `log` facade, as eframe, winit and wgpu already
//! do; before this module nothing installed a logger, so their warnings
//! (GPU errors among them) were dropped. One small logger of our own, not a
//! framework crate, writes each line to stderr and to a per-user file:
//!
//!   Windows  %LOCALAPPDATA%\Patina\patina.log
//!   Linux    $XDG_STATE_HOME/patina/patina.log, else ~/.local/state/patina/
//!
//! The previous run is kept as `patina.log.1`. An instance started while
//! another runs (`patina.lock` is taken) appends to the same file, its
//! lines tagged `#<pid>`, instead of moving the running one's log away.
//! Release builds on Windows have no console (see main.rs), so the file is
//! where their log is; the help overlay shows its path and opens its folder.
//!
//! The directory is chosen by which variables are set, not by `cfg`, so the
//! code is the same on every platform; only showing the file differs.

use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Patina's own lines from Info up; dependencies' from Warn up, since wgpu
/// and winit are chatty at Info.
const OWN_LEVEL: log::LevelFilter = log::LevelFilter::Info;
const DEP_LEVEL: log::LevelFilter = log::LevelFilter::Warn;

struct Logger {
    start: Instant,
    file: Option<Mutex<File>>,
    /// `#<pid> ` on every line of an instance that shares the file with an
    /// earlier one still running; empty for the instance that owns it.
    tag: String,
    /// `patina.lock`, held for as long as the logger lives (the process):
    /// while it is, no later instance starts the file afresh under this one.
    _lock: Option<File>,
}

/// The log file this process writes, set once by `init`.
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// The log file this process writes, if it has one.
pub fn path() -> Option<&'static Path> {
    PATH.get().map(PathBuf::as_path)
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level()
            <= if is_own(m.target()) {
                OWN_LEVEL
            } else {
                DEP_LEVEL
            }
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = self.format(r);
        // A missing console (a release build on Windows) or a full disk must
        // never stop the app over a log line.
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.flush();
            }
        }
    }
}

impl Logger {
    /// `[  12.345] message` for Patina, as before; dependencies add their
    /// level and target, which is what makes their lines useful.
    fn format(&self, r: &log::Record) -> String {
        let t = self.start.elapsed();
        if is_own(r.target()) {
            format!(
                "[{:6}.{:03}] {}{}\n",
                t.as_secs(),
                t.subsec_millis(),
                self.tag,
                r.args()
            )
        } else {
            format!(
                "[{:6}.{:03}] {}{} {}: {}\n",
                t.as_secs(),
                t.subsec_millis(),
                self.tag,
                r.level(),
                r.target(),
                r.args()
            )
        }
    }
}

fn is_own(target: &str) -> bool {
    target == "Patina" || target.starts_with("Patina::")
}

/// Install the logger and a panic hook that logs the panic, so a crash in a
/// build without a console still leaves its reason in the file. Call once,
/// first thing in `main`.
pub fn init() {
    let log = log_dir(|k| std::env::var_os(k)).and_then(|dir| open_log(&dir));
    let path = log.as_ref().map(|l| l.path.display().to_string());
    let tag = match &log {
        Some(l) if l.lock.is_none() => format!("#{} ", std::process::id()),
        _ => String::new(),
    };
    let (file, lock) = match log {
        Some(l) => {
            let _ = PATH.set(l.path);
            (Some(Mutex::new(l.file)), l.lock)
        }
        None => (None, None),
    };
    let logger = Logger {
        start: Instant::now(),
        file,
        tag,
        _lock: lock,
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(OWN_LEVEL.max(DEP_LEVEL));
    }
    log::info!(
        "Patina {} — log file: {}",
        env!("CARGO_PKG_VERSION"),
        path.as_deref().unwrap_or("none")
    );

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        log::logger().flush();
        default_hook(info);
    }));
}

/// Show `path` in the system's file manager: selected in Explorer, its
/// folder elsewhere. Returns once the file manager has started.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    let mut command = reveal_command(path);
    let mut child = command.spawn()?;
    // Explorer exits 1 even when it worked; the wait only reaps the child.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(windows)]
fn reveal_command(path: &Path) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    // Explorer parses `/select,"path"` itself; std's quoting of the whole
    // argument would put the quote before `/select`, which it ignores.
    let mut c = std::process::Command::new("explorer");
    c.raw_arg(format!("/select,\"{}\"", path.display()));
    c
}

#[cfg(not(windows))]
fn reveal_command(path: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("xdg-open");
    c.arg(path.parent().unwrap_or(path));
    c
}

/// The per-user directory for the log, from the environment `var` reads.
fn log_dir(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |k| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    set("LOCALAPPDATA")
        .map(|d| d.join("Patina"))
        .or_else(|| set("XDG_STATE_HOME").map(|d| d.join("patina")))
        .or_else(|| set("HOME").map(|d| d.join(".local").join("state").join("patina")))
}

/// An open log file, and the lock that makes this process its owner.
struct LogFile {
    file: File,
    path: PathBuf,
    /// `patina.lock`, held exclusively; `None` when another instance holds
    /// it and this one appends to that instance's file.
    lock: Option<File>,
}

/// Open `patina.log` in `dir`. The first instance takes `patina.lock`,
/// starts the file afresh and keeps the last run as `patina.log.1`; one
/// started while it runs finds the lock taken and appends instead, so it
/// never moves a log another instance is writing. None if the directory
/// cannot be written; the app then logs to stderr only.
fn open_log(dir: &Path) -> Option<LogFile> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join("patina.log");
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("patina.lock"))
        .ok()
        .filter(|f| f.try_lock().is_ok());
    if lock.is_some() {
        let _ = std::fs::rename(&path, dir.join("patina.log.1"));
        File::create(&path).ok()?;
    }
    // Append mode for the owner too: a handle that writes at its own offset
    // would write over the lines a later instance appended.
    let file = File::options().create(true).append(true).open(&path).ok()?;
    Some(LogFile { file, path, lock })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_log_goes_to_the_platform_state_directory() {
        let windows = [
            ("LOCALAPPDATA", r"C:\Users\a\AppData\Local"),
            ("HOME", r"C:\Users\a"),
        ];
        assert_eq!(
            log_dir(env(&windows)),
            Some(PathBuf::from(r"C:\Users\a\AppData\Local").join("Patina"))
        );

        let xdg = [("XDG_STATE_HOME", "/home/a/.state"), ("HOME", "/home/a")];
        assert_eq!(
            log_dir(env(&xdg)),
            Some(PathBuf::from("/home/a/.state/patina"))
        );

        let plain = [("XDG_STATE_HOME", ""), ("HOME", "/home/a")];
        assert_eq!(
            log_dir(env(&plain)),
            Some(PathBuf::from("/home/a/.local/state/patina"))
        );

        assert_eq!(log_dir(env(&[])), None);
    }

    #[test]
    fn each_run_starts_a_fresh_file_and_keeps_the_last() {
        let dir = std::env::temp_dir().join(format!("patina-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut first = open_log(&dir).unwrap();
        assert!(first.lock.is_some());
        first.file.write_all(b"first run\n").unwrap();
        drop(first);
        let mut second = open_log(&dir).unwrap();
        assert!(second.lock.is_some(), "the first run's lock went with it");
        second.file.write_all(b"second run\n").unwrap();
        let path = second.path.clone();
        drop(second);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second run\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("patina.log.1")).unwrap(),
            "first run\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An instance started while another runs appends to its file instead
    /// of moving it to `patina.log.1` under it, and the earlier run's log
    /// in `patina.log.1` survives.
    #[test]
    fn a_second_instance_shares_the_running_one_s_file() {
        let dir = std::env::temp_dir().join(format!("patina-log-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("patina.log"), "yesterday\n").unwrap();

        let mut running = open_log(&dir).unwrap();
        running.file.write_all(b"running\n").unwrap();
        let mut second = open_log(&dir).unwrap();
        assert!(second.lock.is_none(), "the lock is the running one's");
        second.file.write_all(b"#2 second\n").unwrap();
        running.file.write_all(b"running still\n").unwrap();
        drop((running, second));

        assert_eq!(
            std::fs::read_to_string(dir.join("patina.log")).unwrap(),
            "running\n#2 second\nrunning still\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("patina.log.1")).unwrap(),
            "yesterday\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_reach_the_file_and_dependencies_are_quieter() {
        let dir = std::env::temp_dir().join(format!("patina-log-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = open_log(&dir).unwrap();
        let path = log.path;
        let logger = Logger {
            start: Instant::now(),
            file: Some(Mutex::new(log.file)),
            tag: String::new(),
            _lock: log.lock,
        };
        let rec = |target, level, msg| {
            use log::Log;
            logger.log(
                &log::Record::builder()
                    .target(target)
                    .level(level)
                    .args(format_args!("{msg}"))
                    .build(),
            );
        };

        rec("Patina", log::Level::Info, "[save] written");
        rec("wgpu_core::device", log::Level::Info, "chatty");
        rec("wgpu_core::device", log::Level::Warn, "device lost");
        log::Log::flush(&logger);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("] [save] written\n"), "{text}");
        assert!(!text.contains("chatty"), "{text}");
        assert!(
            text.contains("WARN wgpu_core::device: device lost"),
            "{text}"
        );
        drop(logger);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
