//! Running child processes without freezing the window.
//!
//! While `pnputil` runs, or while a big folder is read or copied, the window must not freeze ("Not
//! responding"). The output of child processes is read on helper threads while the UI thread keeps pumping
//! messages through the hook set with `set_pump`, so the (disabled) window keeps repainting.

use std::cell::Cell;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

static PUMP: OnceLock<fn()> = OnceLock::new();

thread_local! {
    static LAST_PUMP: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Registers the function that processes pending window messages (Application.DoEvents).
pub fn set_pump(pump: fn()) {
    let _ = PUMP.set(pump);
}

pub fn pump() {
    if let Some(f) = PUMP.get() {
        f();
    }
}

/// Like `pump`, but at most about 20 times per second: for loops that touch thousands of files.
pub fn pump_throttled() {
    let due = LAST_PUMP.with(|last| {
        let now = Instant::now();
        match last.get() {
            Some(before) if now.duration_since(before) < Duration::from_millis(50) => false,
            _ => {
                last.set(Some(now));
                true
            }
        }
    });
    if due {
        pump();
    }
}

fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn read_all<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = reader.read_to_end(&mut buffer);
        buffer
    })
}

fn wait_pumping(child: &mut std::process::Child) -> io::Result<i32> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code().unwrap_or(-1));
        }
        pump();
        thread::sleep(Duration::from_millis(15));
    }
}

/// Runs a command; stdout and stderr go into ONE pipe, so the lines keep the order in which the
/// program wrote them (`2>&1` in the script).
pub fn run_merged(mut command: Command) -> io::Result<(i32, Vec<u8>)> {
    hide_window(&mut command);
    let (reader, writer) = io::pipe()?;
    command.stdin(Stdio::null()).stdout(writer.try_clone()?).stderr(writer);
    let mut child = command.spawn()?;
    // The Command still holds the write ends; they must be closed or the reader never sees the end.
    drop(command);
    let output = read_all(reader);
    let exit_code = wait_pumping(&mut child)?;
    Ok((exit_code, output.join().unwrap_or_default()))
}

/// Hex of the first bytes at and after the first non-ASCII byte of a console output, e.g. "61 E1 20 70", or
/// None when the output is plain ASCII. Used by the diagnostic line of the log.
pub fn first_non_ascii_bytes(bytes: &[u8]) -> Option<String> {
    let start = bytes.iter().position(|b| !b.is_ascii())?;
    let from = start.saturating_sub(4);
    let to = (start + 8).min(bytes.len());
    Some(bytes[from..to].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "))
}

/// Text written by a console program. The encoding is not the same on every Windows, so it is decided from
/// the bytes, in this order:
///  1. valid UTF-8 (plain ASCII is valid UTF-8 too);
///  2. the ANSI code page, then the default ANSI code page of the user's language (1252 in Brazil), then the
///     OEM code page, then the default OEM code page of the language (850), each only when it converts
///     WITHOUT invalid bytes;
///  3. last resort: UTF-8 with a replacement character for every invalid byte.
/// The ANSI pages come before the OEM ones because that is what pnputil writes: on a Windows whose system
/// code pages are UTF-8 (65001) it still writes Windows-1252, the ANSI page of the language.
/// Only the text shown and logged depends on this; success is judged by the exit code.
#[cfg(windows)]
pub fn decode_console_output(bytes: &[u8]) -> String {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{
        GetLocaleInfoEx, MultiByteToWideChar, CP_ACP, CP_OEMCP, LOCALE_IDEFAULTANSICODEPAGE, LOCALE_IDEFAULTCODEPAGE,
        MB_ERR_INVALID_CHARS,
    };

    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }

    let decode_with = |code_page: u32| -> Option<String> {
        unsafe {
            let needed = MultiByteToWideChar(code_page, MB_ERR_INVALID_CHARS, bytes, None);
            if needed <= 0 {
                return None;
            }
            let mut wide = vec![0u16; needed as usize];
            let written = MultiByteToWideChar(code_page, MB_ERR_INVALID_CHARS, bytes, Some(&mut wide));
            if written <= 0 {
                return None;
            }
            Some(String::from_utf16_lossy(&wide[..written as usize]))
        }
    };

    // A default code page of the user's language, from the locale: it does not change when the system-wide
    // code pages are UTF-8.
    let locale_page = |kind: u32| -> Option<u32> {
        unsafe {
            let mut buffer = [0u16; 16];
            let length = GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buffer));
            if length > 1 {
                String::from_utf16_lossy(&buffer[..(length as usize - 1)]).trim().parse::<u32>().ok()
            } else {
                None
            }
        }
    };

    let candidates = [
        Some(CP_ACP),
        locale_page(LOCALE_IDEFAULTANSICODEPAGE),
        Some(CP_OEMCP),
        locale_page(LOCALE_IDEFAULTCODEPAGE),
    ];
    for code_page in candidates.into_iter().flatten() {
        if let Some(text) = decode_with(code_page) {
            return text;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// "OEM code page 850, ANSI code page 1252" for the diagnostic line of the log.
#[cfg(windows)]
pub fn code_page_summary() -> String {
    use windows::Win32::Globalization::{GetACP, GetOEMCP};
    unsafe { format!("OEM code page {}, ANSI code page {}", GetOEMCP(), GetACP()) }
}

#[cfg(not(windows))]
pub fn decode_console_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(not(windows))]
pub fn code_page_summary() -> String {
    String::from("not Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_bytes_around_the_first_accent() {
        assert_eq!(first_non_ascii_bytes(b"plain text"), None);
        assert_eq!(first_non_ascii_bytes(b"Utilit\xE1rio PnP"), Some("69 6C 69 74 E1 72 69 6F 20 50 6E 50".to_string()));
        assert_eq!(first_non_ascii_bytes(b"\xC3\xA1"), Some("C3 A1".to_string()));
    }
}
