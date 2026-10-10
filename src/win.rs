//! Thin Win32 helpers: message boxes, the message pump, folder and save dialogs, clipboard, Explorer,
//! system folders, administrator check.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use windows::core::{w, Interface, HSTRING, PCWSTR, PWSTR};
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::Security::{
    AllocateAndInitializeSid, CheckTokenMembership, FreeSid, PSID, SECURITY_NT_AUTHORITY, SID_IDENTIFIER_AUTHORITY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};
use windows::Win32::System::Ole::{OleInitialize, CF_UNICODETEXT};
use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialogCustomize, IFileOpenDialog, IFileSaveDialog, SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
    BROWSEINFOW, FOS_FORCEFILESYSTEM, FOS_NOREADONLYRETURN, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MessageBoxW, PeekMessageW, PostQuitMessage, TranslateMessage, MB_DEFBUTTON2,
    MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MESSAGEBOX_STYLE, MSG,
    PM_REMOVE, WM_QUIT, IDYES,
};

use crate::model::APP_NAME;

/// A NUL-terminated UTF-16 copy of a string.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Information,
    Warning,
    Error,
    Question,
}

fn icon_style(icon: Icon) -> MESSAGEBOX_STYLE {
    match icon {
        Icon::Information => MB_ICONINFORMATION,
        Icon::Warning => MB_ICONWARNING,
        Icon::Error => MB_ICONERROR,
        Icon::Question => MB_ICONQUESTION,
    }
}

/// Shows an information, warning or error message box (Show-Message). Like MessageBox.Show without an
/// owner, the box belongs to the active window of this thread.
pub fn show_message(text: &str, icon: Icon) {
    unsafe {
        let owner = GetActiveWindow();
        MessageBoxW(Some(owner), &HSTRING::from(text), &HSTRING::from(APP_NAME), MB_OK | icon_style(icon));
    }
}

/// Asks a Yes/No question (Confirm-Action). The default button is No, so pressing Enter never
/// confirms something by accident.
pub fn confirm_action(text: &str, icon: Icon) -> bool {
    unsafe {
        let owner = GetActiveWindow();
        let answer = MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from(APP_NAME),
            MB_YESNO | icon_style(icon) | MB_DEFBUTTON2,
        );
        answer == IDYES
    }
}

/// Application.DoEvents: processes the messages that are waiting in the queue.
pub fn do_events() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                // Give the quit message back to the main loop.
                PostQuitMessage(msg.wParam.0 as i32);
                return;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Starts a single-threaded apartment on this thread (needed by the shell dialogs).
pub fn init_sta() {
    unsafe {
        // OleInitialize = CoInitializeEx(STA) + what the "new style" folder dialog needs.
        if OleInitialize(None).is_err() {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
    }
}

/// The program runs elevated: member of the built-in Administrators group in the current token.
pub fn is_administrator() -> bool {
    unsafe {
        let mut authority = SID_IDENTIFIER_AUTHORITY { Value: SECURITY_NT_AUTHORITY.Value };
        let mut sid = PSID::default();
        // S-1-5-32-544 = BUILTIN\Administrators
        if AllocateAndInitializeSid(&mut authority, 2, 32, 544, 0, 0, 0, 0, 0, 0, &mut sid).is_err() {
            return false;
        }
        let mut is_member = BOOL(0);
        let ok = CheckTokenMembership(None, sid, &mut is_member).is_ok();
        FreeSid(sid);
        ok && is_member.as_bool()
    }
}

/// True when this is a 32-bit process on 64-bit Windows (the case the script refuses to run in).
pub fn is_32bit_process_on_64bit_windows() -> bool {
    unsafe {
        let mut wow64 = BOOL(0);
        IsWow64Process(GetCurrentProcess(), &mut wow64).is_ok() && wow64.as_bool()
    }
}

/// Puts text on the clipboard (Clipboard.SetText).
pub fn set_clipboard_text(owner: HWND, text: &str) -> Result<()> {
    unsafe {
        OpenClipboard(Some(owner)).map_err(|e| anyhow!("The clipboard could not be opened: {e}"))?;
        let result = (|| -> Result<()> {
            EmptyClipboard().map_err(|e| anyhow!("The clipboard could not be emptied: {e}"))?;
            let data = wide(text);
            let bytes = data.len() * 2;
            let global: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| anyhow!("Out of memory: {e}"))?;
            let pointer = GlobalLock(global) as *mut u16;
            if pointer.is_null() {
                let _ = GlobalFree(Some(global));
                return Err(anyhow!("The clipboard memory could not be locked."));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), pointer, data.len());
            let _ = GlobalUnlock(global);
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).is_err() {
                let _ = GlobalFree(Some(global));
                return Err(anyhow!("The text could not be placed on the clipboard."));
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// The Windows system folder (C:\Windows\System32), asked from Windows.
pub fn system_directory() -> PathBuf {
    let mut buffer = [0u16; 520];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return PathBuf::from("C:\\Windows\\System32");
    }
    PathBuf::from(OsString::from_wide(&buffer[..length]))
}

/// The Windows folder (C:\Windows), asked from Windows.
pub fn windows_directory() -> PathBuf {
    let mut buffer = [0u16; 520];
    let length = unsafe { GetWindowsDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return PathBuf::from("C:\\Windows");
    }
    PathBuf::from(OsString::from_wide(&buffer[..length]))
}

/// Starts a program with its full path; the arguments are passed as they are.
pub fn start_program(program: &std::path::Path, arguments: &str) -> Result<()> {
    std::process::Command::new(program)
        .raw_arg(arguments)
        .spawn()
        .map(|_child| ())
        .map_err(|e| anyhow!("{} could not be started: {}", program.display(), crate::fsops::clean_io(&e)))
}

/// Opens Explorer (from the Windows folder) with the given arguments.
pub fn start_explorer(arguments: &str) -> Result<()> {
    start_program(&windows_directory().join("explorer.exe"), arguments)
}

/// Opens the Device Manager properties of a device (the same call Driver Store Explorer makes).
pub fn open_device_properties(instance_id: &str) -> Result<()> {
    if instance_id.is_empty() || instance_id.contains('"') {
        return Err(anyhow!("This device has no usable instance ID."));
    }
    start_program(
        &system_directory().join("rundll32.exe"),
        &format!("devmgr.dll,DeviceProperties_RunDLL /MachineName \"\" /DeviceID \"{instance_id}\""),
    )
}

/// FolderBrowserDialog with a Description: the classic tree dialog of .NET Framework.
/// Returns None when the user cancels.
pub fn browse_for_folder(owner: HWND, description: &str) -> Option<PathBuf> {
    unsafe {
        let title = wide(description);
        let mut display = [0u16; 260];
        let info = BROWSEINFOW {
            hwndOwner: owner,
            pszDisplayName: PWSTR(display.as_mut_ptr()),
            lpszTitle: PCWSTR(title.as_ptr()),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            ..Default::default()
        };
        let item = SHBrowseForFolderW(&info);
        if item.is_null() {
            return None;
        }
        let mut path = [0u16; 260];
        let ok = SHGetPathFromIDListW(item, &mut path).as_bool();
        CoTaskMemFree(Some(item as *const _));
        if !ok {
            return None;
        }
        let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        Some(PathBuf::from(OsString::from_wide(&path[..end])))
    }
}

/// The folder dialog of Windows Vista and later with one check box under the folder list. Returns the folder
/// and whether the box was ticked (it starts unticked), or None when the user cancels. If that dialog cannot
/// be created, the classic folder dialog is used instead (without the box) and the reason goes to the log.
pub fn browse_for_folder_with_option(owner: HWND, title: &str, option_label: &str) -> Option<(PathBuf, bool)> {
    match pick_folder_with_option(owner, title, option_label) {
        Ok(result) => result,
        Err(error) => {
            crate::applog::warn(&format!("The folder dialog with the option could not be used ({error}); the classic dialog is used."));
            browse_for_folder(owner, title).map(|path| (path, false))
        }
    }
}

fn pick_folder_with_option(owner: HWND, title: &str, option_label: &str) -> Result<Option<(PathBuf, bool)>> {
    const OPTION_ID: u32 = 1;
    /// HRESULT_FROM_WIN32(ERROR_CANCELLED)
    const CANCELLED: u32 = 0x8007_04C7;
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| anyhow!("The folder dialog could not be created: {e}"))?;
        let options = dialog.GetOptions().map_err(|e| anyhow!("{e}"))?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).map_err(|e| anyhow!("{e}"))?;
        dialog.SetTitle(&HSTRING::from(title)).map_err(|e| anyhow!("{e}"))?;
        let custom: IFileDialogCustomize = dialog.cast().map_err(|e| anyhow!("{e}"))?;
        custom.AddCheckButton(OPTION_ID, &HSTRING::from(option_label), false).map_err(|e| anyhow!("{e}"))?;

        if let Err(error) = dialog.Show(Some(owner)) {
            if error.code().0 as u32 == CANCELLED {
                return Ok(None);
            }
            return Err(anyhow!("{error}"));
        }
        let result = dialog.GetResult().map_err(|e| anyhow!("{e}"))?;
        let name = result.GetDisplayName(SIGDN_FILESYSPATH).map_err(|e| anyhow!("{e}"))?;
        let path = PathBuf::from(OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.0 as *const _));
        let ticked = custom.GetCheckButtonState(OPTION_ID).map_err(|e| anyhow!("{e}"))?.as_bool();
        Ok(Some((path, ticked)))
    }
}

/// SaveFileDialog (the Vista-style dialog WinForms uses). `filters` are (title, pattern) pairs; the
/// first one is selected. Returns None when the user cancels.
pub fn save_file_dialog(
    owner: HWND,
    title: &str,
    filters: &[(&str, &str)],
    default_extension: &str,
    file_name: &str,
) -> Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| anyhow!("The save dialog could not be created: {e}"))?;

        let storage: Vec<(HSTRING, HSTRING)> =
            filters.iter().map(|(n, p)| (HSTRING::from(*n), HSTRING::from(*p))).collect();
        let specs: Vec<COMDLG_FILTERSPEC> = storage
            .iter()
            .map(|(n, p)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(p.as_ptr()) })
            .collect();
        dialog.SetFileTypes(&specs).map_err(|e| anyhow!("{e}"))?;
        dialog.SetFileTypeIndex(1).map_err(|e| anyhow!("{e}"))?;
        dialog.SetDefaultExtension(&HSTRING::from(default_extension)).map_err(|e| anyhow!("{e}"))?;
        dialog.SetTitle(&HSTRING::from(title)).map_err(|e| anyhow!("{e}"))?;
        dialog.SetFileName(&HSTRING::from(file_name)).map_err(|e| anyhow!("{e}"))?;
        let options = dialog.GetOptions().map_err(|e| anyhow!("{e}"))?;
        dialog
            .SetOptions(options | FOS_OVERWRITEPROMPT | FOS_PATHMUSTEXIST | FOS_NOREADONLYRETURN | FOS_FORCEFILESYSTEM)
            .map_err(|e| anyhow!("{e}"))?;

        if dialog.Show(Some(owner)).is_err() {
            // The user cancelled (HRESULT_FROM_WIN32(ERROR_CANCELLED)).
            return Ok(None);
        }
        let result = dialog.GetResult().map_err(|e| anyhow!("{e}"))?;
        let name = result.GetDisplayName(SIGDN_FILESYSPATH).map_err(|e| anyhow!("{e}"))?;
        let path = PathBuf::from(OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(Some(path))
    }
}

#[allow(dead_code)]
pub fn unused(_: LPARAM, _: WPARAM) {
    let _ = w!("");
    let _ = CloseHandle;
}
