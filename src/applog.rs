//! One log file per run, `.\log\DriverStoreManager_<start time>.log`, next to the program.
//!
//! Logging is best-effort: a failure to write must never crash the program (for example when the disk
//! is full while a removal is running), but the user is told about it at the end of the current action.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::model::APP_FILE_NAME;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

struct Paths {
    app_dir: PathBuf,
    log_dir: PathBuf,
    log_file: PathBuf,
    backup_root: PathBuf,
}

static PATHS: OnceLock<Paths> = OnceLock::new();
static LOG_BROKEN: AtomicBool = AtomicBool::new(false);

/// Current time as text, always formatted the same way on every Windows.
pub fn timestamp(format: &str) -> String {
    crate::date::format_now(format)
}

/// Default file-name stamp: yyyyMMdd_HHmmss.
pub fn file_stamp() -> String {
    timestamp("%Y%m%d_%H%M%S")
}

/// The folder of the program (.exe). Log, backup and settings are kept next to it.
pub fn app_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Fixes the paths for this run. The log file is named after the moment the program started.
pub fn init() {
    let app_dir = app_directory();
    let log_dir = app_dir.join("log");
    let log_file = log_dir.join(format!("{}_{}.log", APP_FILE_NAME, file_stamp()));
    let backup_root = app_dir.join("backup");
    let _ = PATHS.set(Paths { app_dir, log_dir, log_file, backup_root });
}

/// Creates the log folder and refuses it (or a planted log file) when it is a link or junction.
pub fn prepare() -> anyhow::Result<()> {
    crate::fsops::ensure_plain_dir(log_dir())?;
    let file = log_file();
    if file.exists() && crate::fsops::is_reparse_point(file).unwrap_or(true) {
        anyhow::bail!("The log file '{}' is a link, not a normal file. It is refused for safety.", file.display());
    }
    Ok(())
}

fn paths() -> &'static Paths {
    PATHS.get().expect("applog::init() must run first")
}

pub fn app_dir() -> &'static Path {
    &paths().app_dir
}
pub fn log_dir() -> &'static Path {
    &paths().log_dir
}
pub fn log_file() -> &'static Path {
    &paths().log_file
}
pub fn backup_root() -> &'static Path {
    &paths().backup_root
}

/// True when a log line could not be written (reported in the summary of an action).
pub fn is_broken() -> bool {
    LOG_BROKEN.load(Ordering::Relaxed)
}

/// Appends one line to the log file of this run: `2026-09-03 10:15:00 [INFO ] text`.
pub fn write(level: Level, message: &str) {
    let line = format!("{} [{:<5}] {}\r\n", timestamp("%Y-%m-%d %H:%M:%S"), level.as_str(), message);
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().create(true).append(true).open(log_file())?;
        // Add-Content -Encoding UTF8 writes a byte order mark when it starts a new file.
        if file.metadata()?.len() == 0 {
            file.write_all(&[0xEF, 0xBB, 0xBF])?;
        }
        file.write_all(line.as_bytes())
    })();
    if result.is_err() {
        LOG_BROKEN.store(true, Ordering::Relaxed);
    }
}

pub fn info(message: &str) {
    write(Level::Info, message);
}
pub fn warn(message: &str) {
    write(Level::Warn, message);
}
pub fn error(message: &str) {
    write(Level::Error, message);
}
