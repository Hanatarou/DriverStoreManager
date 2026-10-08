//! Settings kept between runs: `DriverStoreManager.ini` next to the program.
//!
//! The file is plain text (`Key=Value`, one per line). It is read defensively: unknown keys are ignored, and
//! any value that is missing or out of range falls back to its default, so a damaged or edited file can
//! never stop the program or put it in an odd state.

use std::fs;
use std::path::Path;

use crate::model::DEFAULT_GROUP_MODE;

/// Largest settings file that is read. Anything beyond is ignored.
const MAX_BYTES: usize = 16 * 1024;

/// Position and size of the window when it is not maximized, in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Options > Back up packages before removing
    pub back_up: bool,
    /// Options > Include boot-critical packages in the automatic selections
    pub include_boot_critical: bool,
    /// View > Show only old packages
    pub old_only: bool,
    /// View > Show only packages used only by disconnected devices
    pub disconnected_only: bool,
    /// View > Show only packages used by devices with a problem
    pub problem_only: bool,
    /// Index into GROUP_MODES
    pub group_mode: usize,
    /// Index into COLUMN_DEFINITIONS
    pub sort_column: usize,
    pub sort_descending: bool,
    pub window: Option<WindowState>,
    /// Width of each column in 96-dpi pixels; empty = the default widths. One entry per column otherwise.
    pub column_widths: Vec<Option<i32>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            back_up: true,
            include_boot_critical: false,
            old_only: false,
            disconnected_only: false,
            problem_only: false,
            group_mode: DEFAULT_GROUP_MODE,
            sort_column: 0,
            sort_descending: false,
            window: None,
            column_widths: Vec::new(),
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn parse_index(value: &str, count: usize) -> Option<usize> {
    value.trim().parse::<usize>().ok().filter(|&v| v < count)
}

fn parse_int(value: &str, min: i32, max: i32) -> Option<i32> {
    value.trim().parse::<i32>().ok().filter(|v| (min..=max).contains(v))
}

/// Reads the text of a settings file. `group_modes` and `columns` are the sizes of the lists the indexes refer to.
pub fn parse(text: &str, group_modes: usize, columns: usize) -> Settings {
    let mut settings = Settings::default();
    let (mut left, mut top, mut width, mut height) = (None, None, None, None);
    let mut maximized = false;
    let mut widths: Vec<Option<i32>> = vec![None; columns];

    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if let Some(index) = key.strip_prefix("ColumnWidth").and_then(|n| n.parse::<usize>().ok()) {
            if index < columns {
                widths[index] = parse_int(value, 30, 3000);
            }
            continue;
        }
        match key {
            "BackUpBeforeRemoving" => settings.back_up = parse_bool(value).unwrap_or(settings.back_up),
            "IncludeBootCritical" => settings.include_boot_critical = parse_bool(value).unwrap_or(false),
            "ShowOnlyOld" => settings.old_only = parse_bool(value).unwrap_or(false),
            "ShowOnlyDisconnected" => settings.disconnected_only = parse_bool(value).unwrap_or(false),
            "ShowOnlyProblems" => settings.problem_only = parse_bool(value).unwrap_or(false),
            "GroupBy" => settings.group_mode = parse_index(value, group_modes).unwrap_or(DEFAULT_GROUP_MODE),
            "SortColumn" => settings.sort_column = parse_index(value, columns).unwrap_or(0),
            "SortDescending" => settings.sort_descending = parse_bool(value).unwrap_or(false),
            "WindowLeft" => left = parse_int(value, -32000, 32000),
            "WindowTop" => top = parse_int(value, -32000, 32000),
            "WindowWidth" => width = parse_int(value, 400, 20000),
            "WindowHeight" => height = parse_int(value, 300, 20000),
            "WindowMaximized" => maximized = parse_bool(value).unwrap_or(false),
            _ => {}
        }
    }
    if widths.iter().any(|w| w.is_some()) {
        settings.column_widths = widths;
    }
    if let (Some(left), Some(top), Some(width), Some(height)) = (left, top, width, height) {
        settings.window = Some(WindowState { left, top, width, height, maximized });
    }
    settings
}

/// The text written to the settings file.
pub fn to_text(settings: &Settings) -> String {
    let flag = |v: bool| if v { "1" } else { "0" };
    let mut text = String::new();
    text.push_str("; DriverStore Manager settings. Edited by the program; invalid values are ignored.\r\n");
    text.push_str("[Options]\r\n");
    text.push_str(&format!("BackUpBeforeRemoving={}\r\n", flag(settings.back_up)));
    text.push_str(&format!("IncludeBootCritical={}\r\n", flag(settings.include_boot_critical)));
    text.push_str(&format!("ShowOnlyOld={}\r\n", flag(settings.old_only)));
    text.push_str(&format!("ShowOnlyDisconnected={}\r\n", flag(settings.disconnected_only)));
    text.push_str(&format!("ShowOnlyProblems={}\r\n", flag(settings.problem_only)));
    text.push_str(&format!("GroupBy={}\r\n", settings.group_mode));
    text.push_str(&format!("SortColumn={}\r\n", settings.sort_column));
    text.push_str(&format!("SortDescending={}\r\n", flag(settings.sort_descending)));
    if settings.column_widths.iter().any(|w| w.is_some()) {
        text.push_str("[Columns]\r\n");
        for (index, width) in settings.column_widths.iter().enumerate() {
            if let Some(width) = width {
                text.push_str(&format!("ColumnWidth{index}={width}\r\n"));
            }
        }
    }
    if let Some(window) = settings.window {
        text.push_str("[Window]\r\n");
        text.push_str(&format!("WindowLeft={}\r\n", window.left));
        text.push_str(&format!("WindowTop={}\r\n", window.top));
        text.push_str(&format!("WindowWidth={}\r\n", window.width));
        text.push_str(&format!("WindowHeight={}\r\n", window.height));
        text.push_str(&format!("WindowMaximized={}\r\n", flag(window.maximized)));
    }
    text
}

/// Where the main window opens. `saved` is the window rectangle of the last run (left, top, width, height) and
/// `work` the work area of the monitor it would appear on (left, top, right, bottom).
///  1. The saved rectangle is kept when it fits entirely inside the work area. `tolerance` pixels may stick
///     out: Windows 10 and 11 give a window an invisible border of a few pixels, so a window placed against the
///     edge of the screen has a rectangle that is slightly outside it.
///  2. Otherwise, and on the first run, the window is 60% of the work area wide and tall, centered.
pub fn fit_window(saved: Option<(i32, i32, i32, i32)>, work: (i32, i32, i32, i32), tolerance: i32) -> (i32, i32, i32, i32) {
    let (work_left, work_top, work_right, work_bottom) = work;
    if let Some((left, top, width, height)) = saved {
        let fits = width > 0
            && height > 0
            && left >= work_left - tolerance
            && top >= work_top - tolerance
            && left + width <= work_right + tolerance
            && top + height <= work_bottom + tolerance;
        if fits {
            return (left, top, width, height);
        }
    }
    let (work_width, work_height) = (work_right - work_left, work_bottom - work_top);
    let width = (work_width * 6 / 10).max(1);
    let height = (work_height * 6 / 10).max(1);
    (work_left + (work_width - width) / 2, work_top + (work_height - height) / 2, width, height)
}

/// Loads the settings. A missing file gives the defaults silently; a file that cannot be read gives the
/// defaults and a message for the log.
pub fn load(path: &Path, group_modes: usize, columns: usize) -> (Settings, Option<String>) {
    match fs::read(path) {
        Ok(bytes) => {
            let slice = &bytes[..bytes.len().min(MAX_BYTES)];
            (parse(&String::from_utf8_lossy(slice), group_modes, columns), None)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(error) => (
            Settings::default(),
            Some(format!("The settings file '{}' could not be read, defaults are used: {}", path.display(), error)),
        ),
    }
}

pub fn save(path: &Path, settings: &Settings) -> anyhow::Result<()> {
    crate::fsops::write_file_safely(path, to_text(settings).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let settings = Settings {
            back_up: false,
            include_boot_critical: true,
            old_only: true,
            disconnected_only: true,
            problem_only: true,
            group_mode: 3,
            sort_column: 6,
            sort_descending: true,
            window: Some(WindowState { left: -10, top: 20, width: 1200, height: 700, maximized: true }),
            column_widths: (0..18).map(|i| if i % 3 == 0 { Some(100 + i as i32) } else { None }).collect(),
        };
        assert_eq!(parse(&to_text(&settings), 6, 18), settings);
        // No window block: no window state.
        let plain = Settings { window: None, ..settings };
        assert_eq!(parse(&to_text(&plain), 6, 18), plain);
    }

    #[test]
    fn bad_values_fall_back_to_defaults() {
        let text = "BackUpBeforeRemoving=maybe\nGroupBy=99\nSortColumn=-1\nShowOnlyOld=2\nShowOnlyDisconnected=x\nShowOnlyProblems=2\nWindowLeft=1\nWindowTop=1\nWindowWidth=5\nWindowHeight=700\nnonsense\nUnknown=1\n";
        let settings = parse(text, 6, 18);
        assert_eq!(settings, Settings::default());
        // The safe default: backups stay on unless the file clearly says 0.
        assert!(parse("BackUpBeforeRemoving=\n", 6, 18).back_up);
        // Column widths: out-of-range index and widths are ignored.
        let widths = parse("ColumnWidth0=50\nColumnWidth99=50\nColumnWidth1=5\nColumnWidth2=99999\nColumnWidthx=5\n", 6, 18).column_widths;
        assert_eq!(widths.len(), 18);
        assert_eq!(widths[0], Some(50));
        assert!(widths[1..].iter().all(|w| w.is_none()));
        assert!(parse("ColumnWidth1=5\n", 6, 18).column_widths.is_empty());
        assert!(!parse("BackUpBeforeRemoving=0\n", 6, 18).back_up);
    }

    #[test]
    fn tolerates_bom_comments_and_spaces() {
        let settings = parse("\u{feff}; comment\r\n[Options]\r\n GroupBy = 2 \r\nSortDescending=1\r\n", 6, 18);
        assert_eq!(settings.group_mode, 2);
        assert!(settings.sort_descending);
    }

    #[test]
    fn window_fit_rule() {
        // Saved on a 1920x1080 screen, opened on a 1024x728 work area: does not fit, 60% centered.
        assert_eq!(fit_window(Some((525, 246, 1640, 999)), (0, 0, 1024, 728), 16), (205, 146, 614, 436));
        // First run: 60% centered.
        assert_eq!(fit_window(None, (0, 0, 1920, 1040), 16), (384, 208, 1152, 624));
        // Fits: kept exactly, not centered.
        assert_eq!(fit_window(Some((100, 50, 1200, 700)), (0, 0, 1920, 1040), 16), (100, 50, 1200, 700));
        // Against the left edge with the invisible border of the window: kept.
        assert_eq!(fit_window(Some((-7, 0, 974, 1040)), (0, 0, 1920, 1040), 16), (-7, 0, 974, 1040));
        // Too far out, or too big in only one direction: not kept.
        assert_ne!(fit_window(Some((-40, 0, 974, 1040)), (0, 0, 1920, 1040), 16), (-40, 0, 974, 1040));
        assert_ne!(fit_window(Some((0, 0, 1200, 1100)), (0, 0, 1920, 1040), 16), (0, 0, 1200, 1100));
        // A monitor to the left of the main one (negative coordinates) and a taskbar at the top.
        assert_eq!(fit_window(None, (-1920, 0, 0, 1040), 16), (-1536, 208, 1152, 624));
        assert_eq!(fit_window(None, (0, 48, 1920, 1080), 16), (384, 254, 1152, 619));
        // Whatever the saved rectangle, the result is never outside the work area unless it was kept.
        let works = [(0, 0, 1920, 1040), (0, 0, 1024, 728), (0, 0, 800, 600), (-1920, 0, 0, 1040), (0, 48, 1920, 1080), (0, 0, 10, 10)];
        for work in works {
            for left in (-3000..3000).step_by(211) {
                for top in (-3000..3000).step_by(211) {
                    for (width, height) in [(1, 1), (400, 300), (1024, 700), (1640, 999), (20000, 20000)] {
                        let saved = (left, top, width, height);
                        let result = fit_window(Some(saved), work, 16);
                        if result != saved {
                            let (l, t, w, h) = result;
                            assert!(l >= work.0 && t >= work.1 && l + w <= work.2 && t + h <= work.3, "{saved:?} on {work:?} -> {result:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = std::env::temp_dir().join(format!("dsm_missing_{}.ini", std::process::id()));
        let (settings, warning) = load(&path, 6, 18);
        assert_eq!(settings, Settings::default());
        assert!(warning.is_none());
    }
}
