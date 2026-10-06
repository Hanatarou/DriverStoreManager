//! Runs pnputil.exe, logs the command and its output, and returns the result.
//! Success is judged by the exit code only (0 = done, 3010 = done, restart required), never by the text,
//! so the program works on every Windows language.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Result};

use crate::applog;
use crate::model::EXIT_CODE_REBOOT_REQUIRED;
use crate::proc;

static ENCODING_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct PnpResult {
    pub exit_code: i32,
    pub output: String,
    pub success: bool,
    pub reboot_required: bool,
}

/// pnputil.exe from the Windows system folder (asked from Windows, not read from an environment variable
/// and not whatever is first in the search path).
#[cfg(windows)]
pub fn pnputil_path() -> PathBuf {
    crate::win::system_directory().join("pnputil.exe")
}

#[cfg(not(windows))]
pub fn pnputil_path() -> PathBuf {
    PathBuf::from("pnputil.exe")
}

pub fn invoke(arguments: &[String]) -> Result<PnpResult> {
    applog::info(&format!("Running: pnputil {}", arguments.join(" ")));

    let mut command = Command::new(pnputil_path());
    command.args(arguments);
    let (exit_code, raw) =
        proc::run_merged(command).map_err(|e| anyhow!("pnputil.exe could not be started: {}", crate::fsops::clean_io(&e)))?;
    let output = proc::decode_console_output(&raw).trim().to_string();
    // One diagnostic line per run when the output has accents, so a wrong decoding can be understood from the log.
    if let Some(bytes) = proc::first_non_ascii_bytes(&raw) {
        if !ENCODING_LOGGED.swap(true, Ordering::Relaxed) {
            applog::info(&format!(
                "pnputil output is not plain ASCII ({}). Raw bytes around the first accent: {}",
                proc::code_page_summary(),
                bytes
            ));
        }
    }

    let success = exit_code == 0 || exit_code == EXIT_CODE_REBOOT_REQUIRED;
    applog::info(&format!(
        "pnputil finished with exit code {} ({}).",
        exit_code,
        if success { "success" } else { "failure" }
    ));
    for line in output.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !line.trim().is_empty() {
            applog::info(&format!("    {line}"));
        }
    }

    Ok(PnpResult { exit_code, output, success, reboot_required: exit_code == EXIT_CODE_REBOOT_REQUIRED })
}
