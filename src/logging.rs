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
//! The previous run is kept as `patina.log.1`. Release builds on Windows
//! have no console (see main.rs), so the file is where their log is. A
//! release still needs a friendlier way to it than a path (docs/roadmap.md).
//!
//! The directory is chosen by which variables are set, not by `cfg`, so the
//! code is the same on every platform.

use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// Patina's own lines from Info up; dependencies' from Warn up, since wgpu
/// and winit are chatty at Info.
const OWN_LEVEL: log::LevelFilter = log::LevelFilter::Info;
const DEP_LEVEL: log::LevelFilter = log::LevelFilter::Warn;

struct Logger {
    start: Instant,
    file: Option<Mutex<File>>,
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
                "[{:6}.{:03}] {}\n",
                t.as_secs(),
                t.subsec_millis(),
                r.args()
            )
        } else {
            format!(
                "[{:6}.{:03}] {} {}: {}\n",
                t.as_secs(),
                t.subsec_millis(),
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
    let file = log_dir(|k| std::env::var_os(k)).and_then(|dir| open_log(&dir));
    let path = file.as_ref().map(|(_, p)| p.display().to_string());
    let logger = Logger {
        start: Instant::now(),
        file: file.map(|(f, _)| Mutex::new(f)),
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

/// The per-user directory for the log, from the environment `var` reads.
fn log_dir(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |k| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    set("LOCALAPPDATA")
        .map(|d| d.join("Patina"))
        .or_else(|| set("XDG_STATE_HOME").map(|d| d.join("patina")))
        .or_else(|| set("HOME").map(|d| d.join(".local").join("state").join("patina")))
}

/// Start `patina.log` in `dir` afresh, keeping the last run as
/// `patina.log.1`. None if the directory cannot be written; the app then
/// logs to stderr only.
fn open_log(dir: &Path) -> Option<(File, PathBuf)> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join("patina.log");
    let _ = std::fs::rename(&path, dir.join("patina.log.1"));
    Some((File::create(&path).ok()?, path))
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

        let (mut f, path) = open_log(&dir).unwrap();
        f.write_all(b"first run\n").unwrap();
        drop(f);
        let (mut f, _) = open_log(&dir).unwrap();
        f.write_all(b"second run\n").unwrap();
        drop(f);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second run\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("patina.log.1")).unwrap(),
            "first run\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_reach_the_file_and_dependencies_are_quieter() {
        let dir = std::env::temp_dir().join(format!("patina-log-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (f, path) = open_log(&dir).unwrap();
        let logger = Logger {
            start: Instant::now(),
            file: Some(Mutex::new(f)),
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
