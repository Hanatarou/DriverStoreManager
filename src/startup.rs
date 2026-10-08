//! Start: log folder, system information, requirements, settings, window, message loop.

use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsDialogMessageW, TranslateAcceleratorW, TranslateMessage, MSG,
};

use crate::model::{APP_NAME, APP_VERSION, MINIMUM_WINDOWS_BUILD};
use crate::native::{self, SystemInfo};
use crate::win::{self, show_message, Icon};
use crate::{app, applog, drvstore, pnputil, proc, ui};

/// Returns a list of problems that prevent the program from working (empty list = all good).
fn requirement_problems(info: &SystemInfo) -> Vec<String> {
    let mut problems = Vec::new();

    if !win::is_administrator() {
        problems.push(
            "Administrator rights are required to change the Driver Store. Start the program with \"Run as administrator\"."
                .to_string(),
        );
    }

    if info.build_number < MINIMUM_WINDOWS_BUILD {
        problems.push(format!(
            "Windows 10 version 1607 (build {MINIMUM_WINDOWS_BUILD}) or later is required; older versions have no 'pnputil /delete-driver'. This computer runs build {}.",
            info.build_number
        ));
    }

    if win::is_32bit_process_on_64bit_windows() {
        problems.push(
            "A 64-bit process is required on 64-bit Windows (a 32-bit process would use the wrong system folders). Use the 64-bit build of the program (x86_64-pc-windows-msvc)."
                .to_string(),
        );
    }

    let pnputil = pnputil::pnputil_path();
    if !pnputil.exists() {
        problems.push(format!(
            "pnputil.exe was not found at '{}'. It is part of Windows 10 and later.",
            pnputil.display()
        ));
    }

    if let Some(problem) = drvstore::requirement_problem() {
        problems.push(problem);
    }
    problems
}

pub fn run() {
    applog::init();
    // File and folder dialogs only work in a single-threaded apartment.
    win::init_sta();
    proc::set_pump(win::do_events);

    if let Err(error) = applog::prepare() {
        show_message(
            &format!(
                "The log folder could not be prepared:\n{}\n\n{}\n\nMove the program to a folder where you can write files.",
                applog::log_dir().display(),
                error
            ),
            Icon::Error,
        );
        std::process::exit(1);
    }
    applog::info(&format!("Session started: {APP_NAME} {APP_VERSION}."));

    let info = match native::get_system_info() {
        Ok(info) => info,
        Err(error) => {
            show_message(
                &format!("Could not read basic system information:\n\n{error}\n\nThe program cannot continue."),
                Icon::Error,
            );
            std::process::exit(1);
        }
    };
    applog::info(&format!(
        "Program folder: {} | User: {}\\{} | Windows: {} (build {})",
        applog::app_dir().display(),
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default(),
        info.caption,
        info.build_number
    ));
    applog::info(&format!("Log file: {}", applog::log_file().display()));
    applog::info(&format!("Backups are saved in: {}\\<time of removal>", applog::backup_root().display()));

    let problems = requirement_problems(&info);
    if !problems.is_empty() {
        for problem in &problems {
            applog::error(problem);
        }
        show_message(&format!("{APP_NAME} cannot start:\n\n{}", problems.join("\n\n")), Icon::Error);
        std::process::exit(1);
    }

    let settings = app::load_settings();
    if let Err(error) = ui::create_main_window(settings.window) {
        applog::error(&format!("The window could not be created: {error}"));
        show_message(&format!("The window could not be created:\n\n{error}"), Icon::Error);
        std::process::exit(1);
    }
    app::apply_settings(&settings);
    ui::show_main_window();

    // Message loop: shortcuts first, then dialog-style keyboard navigation (Tab), then the normal dispatch.
    unsafe {
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if TranslateAcceleratorW(ui::form(), ui::accelerators(), &message) != 0 {
                continue;
            }
            if IsDialogMessageW(ui::form(), &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
