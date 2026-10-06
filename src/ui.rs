//! The window. Everything here is plain Win32 (no GUI toolkit), which is what gives full control over
//! check boxes, ListView groups, row colors, shortcuts and the status bar.
//!
//! The window knows nothing about drivers: it forwards what the user does to `app`, and `app` calls the
//! primitives below to change what is shown.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Result};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, GetMonitorInfoW, GetStockObject, InvalidateRect, MonitorFromPoint, MonitorFromRect, UpdateWindow,
    DEFAULT_GUI_FONT, HBRUSH, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ICC_PROGRESS_CLASS,
    INITCOMMONCONTROLSEX, LVCFMT_LEFT, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVGA_HEADER_LEFT, LVGF_ALIGN,
    LVGF_GROUPID, LVGF_HEADER, LVGROUP, LVIF_GROUPID, LVIF_PARAM, LVIF_STATE, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVIS_STATEIMAGEMASK, LVITEMW,
    LVM_DELETEALLITEMS, LVM_ENABLEGROUPVIEW, LVM_GETCOLUMNWIDTH, LVM_SETCOLUMNWIDTH, LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTGROUP,
    LVM_INSERTITEMW, LVM_REMOVEALLGROUPS, LVM_SETCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVNI_SELECTED,
    LVN_COLUMNCLICK, LVN_ITEMCHANGED, LVS_EX_CHECKBOXES, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_GRIDLINES,
    LVS_REPORT, LVS_SHOWSELALWAYS, NMHDR, NMLISTVIEW, NMLVCUSTOMDRAW, NM_CUSTOMDRAW, PBM_SETPOS, PBM_SETRANGE32,
    PROGRESS_CLASSW, SBARS_SIZEGRIP, SB_GETRECT, SB_SETPARTS, SB_SETTEXTW, STATUSCLASSNAMEW, WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus, VK_F5};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateAcceleratorTableW, ACCEL, ACCEL_VIRT_FLAGS, FCONTROL, FSHIFT, FVIRTKEY, AppendMenuW, CheckMenuItem, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyWindow, GetClientRect, GetCursorPos, GetMenuState, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, IsChild, IsIconic, IsZoomed, SW_SHOWMAXIMIZED, WM_MOVE,
    LoadCursorW, LoadIconW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW, SetCursor,
    SetMenu, ShowWindow, SystemParametersInfoW, TrackPopupMenu, BS_PUSHBUTTON, EN_CHANGE, ES_AUTOHSCROLL,
    HACCEL, HMENU, HTCLIENT, IDC_ARROW, IDC_WAIT, IDI_APPLICATION, MF_BYCOMMAND, MF_CHECKED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MF_UNCHECKED, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SPI_GETWORKAREA, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
    WM_NOTIFY, WM_SETCURSOR, WM_SETFONT, WM_SETREDRAW, WM_SIZE, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, BN_CLICKED, WINDOW_EX_STYLE,
    WINDOW_STYLE, WNDCLASS_STYLES, CS_HREDRAW, CS_VREDRAW, HWND_DESKTOP,
};

use crate::app;
use crate::model::{COLUMN_DEFINITIONS, GROUP_MODES, APP_NAME, APP_VERSION};
use crate::settings::WindowState;
use crate::win::wide;

// ---- Command IDs -----------------------------------------------------------------------------------

pub const ID_REFRESH: u16 = 100;
pub const ID_EXPORT_LIST: u16 = 101;
pub const ID_OPEN_LOG: u16 = 102;
pub const ID_OPEN_BACKUP: u16 = 103;
pub const ID_EXIT: u16 = 104;
pub const ID_OPEN_OFFLINE: u16 = 105;
pub const ID_BACK_ONLINE: u16 = 106;
pub const ID_ADD: u16 = 110;
pub const ID_ADD_INSTALL: u16 = 111;
pub const ID_EXPORT_CHECKED: u16 = 112;
pub const ID_EXPORT_ALL: u16 = 113;
pub const ID_REMOVE: u16 = 114;
pub const ID_RESTORE: u16 = 115;
pub const ID_CHECK_OLD_UNUSED: u16 = 120;
pub const ID_CHECK_OLD_IN_USE: u16 = 121;
pub const ID_CHECK_SHOWN: u16 = 122;
pub const ID_UNCHECK_SHOWN: u16 = 123;
pub const ID_UNCHECK_ALL: u16 = 124;
pub const ID_CHECK_UNUSED: u16 = 125;
pub const ID_INVERT: u16 = 126;
pub const ID_GROUP_BASE: u16 = 200; // + index into GROUP_MODES
pub const ID_OLD_ONLY: u16 = 210;
pub const ID_DISCONNECTED_ONLY: u16 = 211;
pub const ID_OPT_BACKUP: u16 = 220;
pub const ID_OPT_INCLUDE_BOOT: u16 = 221;
pub const ID_HELP_HOW: u16 = 230;
pub const ID_HELP_ABOUT: u16 = 231;
pub const ID_CTX_CHECK_GROUP: u16 = 240;
pub const ID_CTX_UNCHECK_GROUP: u16 = 241;
pub const ID_CTX_OPEN_FOLDER: u16 = 242;
pub const ID_CTX_COPY_PATH: u16 = 243;
pub const ID_CTX_REMOVE_SELECTED: u16 = 244;
pub const ID_CTX_EXPORT_SELECTED: u16 = 245;
pub const ID_CTX_DEVICE_PROPS: u16 = 246;

const ID_LIST: i32 = 1000;
const ID_FILTER: u16 = 1001;
const ID_REMOVE_BUTTON: u16 = 1002;
const ID_STATUS: i32 = 1003;
const ID_PROGRESS: i32 = 1004;
const ID_FILTER_LABEL: i32 = 1005;

/// Posted once after the window is first shown (Form.Shown).
const WM_APP_SHOWN: u32 = WM_APP + 1;

// ---- State shared with the window procedure ---------------------------------------------------------

static IS_BUSY: AtomicBool = AtomicBool::new(false);
static IS_POPULATING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Handles {
    form: HWND,
    list: HWND,
    filter_label: HWND,
    filter_box: HWND,
    remove_button: HWND,
    status: HWND,
    progress: HWND,
    menu_bar: HMENU,
    context_menu: HMENU,
    accelerators: HACCEL,
    dpi: u32,
}

thread_local! {
    static HANDLES: Cell<Option<Handles>> = const { Cell::new(None) };
    /// Background color of each list row (COLORREF), u32::MAX = none. Read by the custom-draw handler.
    static ROW_COLORS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static PROGRESS_VISIBLE: Cell<bool> = const { Cell::new(false) };
    static START_MAXIMIZED: Cell<bool> = const { Cell::new(false) };
    /// The last position and size of the window while it was neither maximized nor minimized, in screen coordinates.
    static NORMAL_RECT: Cell<Option<RECT>> = const { Cell::new(None) };
}

fn handles() -> Handles {
    HANDLES.with(|h| h.get()).expect("the window has not been created yet")
}

pub fn form() -> HWND {
    handles().form
}

pub fn accelerators() -> HACCEL {
    handles().accelerators
}

pub fn is_busy() -> bool {
    IS_BUSY.load(Ordering::SeqCst)
}

pub fn is_populating() -> bool {
    IS_POPULATING.load(Ordering::SeqCst)
}

pub fn set_populating(value: bool) {
    IS_POPULATING.store(value, Ordering::SeqCst);
}

/// 96-dpi pixels -> pixels of this system.
fn scale(px: i32) -> i32 {
    let dpi = HANDLES.with(|h| h.get()).map(|h| h.dpi).unwrap_or(96) as i64;
    ((px as i64 * dpi + 48) / 96) as i32
}

fn lparam_of<T>(value: &T) -> LPARAM {
    LPARAM(value as *const T as isize)
}

fn send(window: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    unsafe { SendMessageW(window, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }
}

// ---- Menu construction -----------------------------------------------------------------------------

struct MenuBuilder {
    menu: HMENU,
}

impl MenuBuilder {
    fn new_popup() -> Result<Self> {
        Ok(MenuBuilder { menu: unsafe { CreatePopupMenu() }.map_err(|e| anyhow!("A menu could not be created (CreatePopupMenu): {e}"))? })
    }
    fn item(&self, id: u16, text: &str) -> &Self {
        unsafe {
            let _ = AppendMenuW(self.menu, MF_STRING, id as usize, PCWSTR(wide(text).as_ptr()));
        }
        self
    }
    fn separator(&self) -> &Self {
        unsafe {
            let _ = AppendMenuW(self.menu, MF_SEPARATOR, 0, PCWSTR::null());
        }
        self
    }
    fn submenu(&self, text: &str, sub: HMENU) -> &Self {
        unsafe {
            let _ = AppendMenuW(self.menu, MF_POPUP | MF_STRING, sub.0 as usize, PCWSTR(wide(text).as_ptr()));
        }
        self
    }
}

fn build_menus() -> Result<(HMENU, HMENU)> {
    // File
    let file = MenuBuilder::new_popup()?;
    file.item(ID_REFRESH, "Re&fresh\tF5")
        .item(ID_EXPORT_LIST, "&Export list...\tCtrl+E")
        .separator()
        .item(ID_OPEN_OFFLINE, "Open &offline Windows image...")
        .item(ID_BACK_ONLINE, "&Return to the running Windows")
        .separator()
        .item(ID_OPEN_LOG, "Open &log folder")
        .item(ID_OPEN_BACKUP, "Open &backup folder")
        .separator()
        .item(ID_EXIT, "E&xit");

    // Drivers
    let drivers = MenuBuilder::new_popup()?;
    drivers
        .item(ID_ADD, "&Add driver package...\tCtrl+N")
        .item(ID_ADD_INSTALL, "Add and &install driver package...\tCtrl+Shift+N")
        .separator()
        .item(ID_EXPORT_CHECKED, "Export &checked driver packages...")
        .item(ID_EXPORT_ALL, "Export a&ll driver packages...")
        .item(ID_RESTORE, "Re&store backup...")
        .separator()
        .item(ID_REMOVE, "&Remove checked driver packages...");

    // Select
    let select = MenuBuilder::new_popup()?;
    select
        .item(ID_CHECK_OLD_UNUSED, "Check &old packages (unused only - safe)")
        .item(ID_CHECK_OLD_IN_USE, "Check old packages (&including in use)")
        .item(ID_CHECK_UNUSED, "Check unused &packages (no device uses them)")
        .separator()
        .item(ID_CHECK_SHOWN, "Check everything &shown")
        .item(ID_UNCHECK_SHOWN, "&Uncheck everything shown")
        .item(ID_INVERT, "Inver&t checks of everything shown")
        .item(ID_UNCHECK_ALL, "Uncheck &all");

    // View > Group by, Show only old packages, Show only packages of disconnected devices
    let group_by = MenuBuilder::new_popup()?;
    for (index, (name, _)) in GROUP_MODES.iter().enumerate() {
        group_by.item(ID_GROUP_BASE + index as u16, name);
    }
    let view = MenuBuilder::new_popup()?;
    view.submenu("&Group by", group_by.menu)
        .item(ID_OLD_ONLY, "Show only &old packages")
        .item(ID_DISCONNECTED_ONLY, "Show only packages used only by &disconnected devices");

    // Options
    let options = MenuBuilder::new_popup()?;
    options
        .item(ID_OPT_BACKUP, "&Back up packages before removing")
        .item(ID_OPT_INCLUDE_BOOT, "Include &boot-critical packages in the automatic selections");

    // Help
    let help = MenuBuilder::new_popup()?;
    help.item(ID_HELP_HOW, "&How it works...").item(ID_HELP_ABOUT, "&About...");

    let bar = unsafe { CreateMenu() }.map_err(|e| anyhow!("The menu bar could not be created (CreateMenu): {e}"))?;
    let top = MenuBuilder { menu: bar };
    top.submenu("&File", file.menu)
        .submenu("&Drivers", drivers.menu)
        .submenu("&Select", select.menu)
        .submenu("&View", view.menu)
        .submenu("&Options", options.menu)
        .submenu("&Help", help.menu);

    // Right-click menu on the list.
    let context = MenuBuilder::new_popup()?;
    context
        .item(ID_CTX_CHECK_GROUP, "Check all in this group")
        .item(ID_CTX_UNCHECK_GROUP, "Uncheck all in this group")
        .separator()
        .item(ID_CTX_REMOVE_SELECTED, "Remove selected packages...")
        .item(ID_CTX_EXPORT_SELECTED, "Export selected packages...")
        .separator()
        .item(ID_CTX_DEVICE_PROPS, "Open device properties")
        .item(ID_CTX_OPEN_FOLDER, "Open package folder")
        .item(ID_CTX_COPY_PATH, "Copy package folder path");

    Ok((bar, context.menu))
}

// Menu check marks -------------------------------------------------------------------------------------

pub fn set_menu_checked(id: u16, checked: bool) {
    let menu = handles().menu_bar;
    unsafe {
        CheckMenuItem(menu, id as u32, (MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED }).0);
    }
}

pub fn menu_checked(id: u16) -> bool {
    let menu = handles().menu_bar;
    let state = unsafe { GetMenuState(menu, id as u32, MF_BYCOMMAND) };
    state != u32::MAX && (state & MF_CHECKED.0) != 0
}

/// CheckOnClick: toggles the check mark and returns the new value.
pub fn toggle_menu_checked(id: u16) -> bool {
    let value = !menu_checked(id);
    set_menu_checked(id, value);
    value
}

// ---- Window creation -------------------------------------------------------------------------------

fn message_font() -> HFONT {
    unsafe {
        let mut metrics = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let ok = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        if ok.is_ok() {
            let font = CreateFontIndirectW(&metrics.lfMessageFont);
            if !font.is_invalid() {
                return font;
            }
        }
        HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
    }
}

unsafe fn child(
    ex_style: u32,
    class: PCWSTR,
    text: &str,
    style: u32,
    parent: HWND,
    id: i32,
    instance: HINSTANCE,
    font: HFONT,
) -> Result<HWND> {
    let title = wide(text);
    let window = CreateWindowExW(
        WINDOW_EX_STYLE(ex_style),
        class,
        PCWSTR(title.as_ptr()),
        WINDOW_STYLE(style),
        0,
        0,
        10,
        10,
        Some(parent),
        Some(HMENU(id as usize as *mut _)),
        Some(instance),
        None,
    )
    .map_err(|e| anyhow!("A window control could not be created: {e}"))?;
    send(window, WM_SETFONT, font.0 as usize, 1);
    Ok(window)
}

/// Resource ID of the program icon (see build.rs / resources.rc).
const ICON_RESOURCE_ID: usize = 1;

/// Work area (left, top, right, bottom) of the monitor that a rectangle is on, or of the nearest one; of the
/// main monitor when there is no rectangle.
fn monitor_work_area(rect: Option<RECT>) -> (i32, i32, i32, i32) {
    unsafe {
        let monitor = match rect {
            Some(r) => MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST),
            None => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
        };
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() && info.rcWork.right > info.rcWork.left && info.rcWork.bottom > info.rcWork.top {
            return (info.rcWork.left, info.rcWork.top, info.rcWork.right, info.rcWork.bottom);
        }
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        if work.right > work.left && work.bottom > work.top {
            (work.left, work.top, work.right, work.bottom)
        } else {
            (0, 0, 1024, 768)
        }
    }
}

/// Creates the main window with all its controls. `saved` is the window position of the last run.
pub fn create_main_window(saved: Option<WindowState>) -> Result<()> {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_PROGRESS_CLASS,
        };
        let _ = InitCommonControlsEx(&icc);

        let dpi = GetDpiForSystem().max(96);
        HANDLES.with(|h| {
            h.set(Some(Handles {
                form: HWND::default(),
                list: HWND::default(),
                filter_label: HWND::default(),
                filter_box: HWND::default(),
                remove_button: HWND::default(),
                status: HWND::default(),
                progress: HWND::default(),
                menu_bar: HMENU::default(),
                context_menu: HMENU::default(),
                accelerators: HACCEL::default(),
                dpi,
            }))
        });

        let instance: HINSTANCE =
            GetModuleHandleW(None).map_err(|e| anyhow!("The program handle could not be read (GetModuleHandle): {e}"))?.into();
        let class_name = w!("DriverStoreManagerWindow");
        // The program icon is embedded as resource 1; the stock icon is only the fallback.
        let icon = LoadIconW(Some(instance), PCWSTR(ICON_RESOURCE_ID as *const u16))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hIcon: icon,
            hIconSm: icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // COLOR_BTNFACE + 1: the standard form background.
            hbrBackground: HBRUSH(16 as *mut _),
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(anyhow!("The window class could not be registered."));
        }

        let (menu_bar, context_menu) = build_menus()?;
        let font = message_font();

        // Where the window opens: as it was, when that fits on the monitor; otherwise 60% of the work area, centered.
        let saved_rect = saved.map(|s| RECT { left: s.left, top: s.top, right: s.left + s.width, bottom: s.top + s.height });
        let (x, y, width, height) = crate::settings::fit_window(
            saved.map(|s| (s.left, s.top, s.width, s.height)),
            monitor_work_area(saved_rect),
            scale(16),
        );
        START_MAXIMIZED.with(|m| m.set(saved.map(|s| s.maximized).unwrap_or(false)));

        let title = wide(&format!("{APP_NAME} {APP_VERSION} - third-party driver packages"));
        let form = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            width,
            height,
            Some(HWND_DESKTOP),
            Some(menu_bar),
            Some(instance),
            None,
        )
        .map_err(|e| anyhow!("The window could not be created: {e}"))?;
        let _ = SetMenu(form, Some(menu_bar));

        // The controls are created while the main handles are still being filled in, so the window
        // procedure must not run its layout yet: it only does so once HANDLES holds the final values.
        let child_style = WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPSIBLINGS.0;

        // List
        let list = child(
            WS_EX_CLIENTEDGE.0,
            WC_LISTVIEWW,
            "",
            child_style | LVS_REPORT | LVS_SHOWSELALWAYS | WS_TABSTOP.0,
            form,
            ID_LIST,
            instance,
            font,
        )?;
        let _ = SetWindowTheme(list, w!("explorer"), PCWSTR::null());
        send(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_GRIDLINES | LVS_EX_CHECKBOXES | LVS_EX_DOUBLEBUFFER) as isize,
        );
        for (index, definition) in COLUMN_DEFINITIONS.iter().enumerate() {
            let mut text = wide(definition.title);
            let column = LVCOLUMNW {
                mask: LVCF_FMT | LVCF_WIDTH | LVCF_TEXT,
                fmt: LVCFMT_LEFT,
                cx: scale(definition.width),
                pszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            send(list, LVM_INSERTCOLUMNW, index, lparam_of(&column).0);
        }

        // Filter bar
        let filter_label = child(0, w!("STATIC"), "Filter:", child_style, form, ID_FILTER_LABEL, instance, font)?;
        let filter_box = child(
            WS_EX_CLIENTEDGE.0,
            w!("EDIT"),
            "",
            child_style | ES_AUTOHSCROLL as u32 | WS_TABSTOP.0,
            form,
            ID_FILTER as i32,
            instance,
            font,
        )?;
        let remove_button = child(
            0,
            w!("BUTTON"),
            "Remove checked...",
            child_style | BS_PUSHBUTTON as u32 | WS_TABSTOP.0,
            form,
            ID_REMOVE_BUTTON as i32,
            instance,
            font,
        )?;

        // Status bar
        let status = child(0, STATUSCLASSNAMEW, "", child_style | SBARS_SIZEGRIP as u32, form, ID_STATUS, instance, font)?;
        let progress = child(0, PROGRESS_CLASSW, "", WS_CHILD.0, status, ID_PROGRESS, instance, font)?;

        // The shortcut keys are a convenience: when the table cannot be created (some minimal Windows setups),
        // the program still starts and everything stays reachable from the menus.
        let accelerators = match CreateAcceleratorTableW(&[
            ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0), key: VK_F5.0, cmd: ID_REFRESH },
            ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | FCONTROL.0), key: b'E' as u16, cmd: ID_EXPORT_LIST },
            ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | FCONTROL.0), key: b'N' as u16, cmd: ID_ADD },
            ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | FCONTROL.0 | FSHIFT.0), key: b'N' as u16, cmd: ID_ADD_INSTALL },
        ]) {
            Ok(table) => table,
            Err(error) => {
                crate::applog::warn(&format!("The shortcut keys are not available (CreateAcceleratorTable): {error}"));
                HACCEL::default()
            }
        };

        HANDLES.with(|h| {
            h.set(Some(Handles {
                form,
                list,
                filter_label,
                filter_box,
                remove_button,
                status,
                progress,
                menu_bar,
                context_menu,
                accelerators,
                dpi,
            }))
        });

        layout();
        Ok(())
    }
}

/// Shows the window and arranges for the Shown event (loading the driver packages).
pub fn show_main_window() {
    let h = handles();
    unsafe {
        let _ = ShowWindow(h.form, if START_MAXIMIZED.with(|m| m.get()) { SW_SHOWMAXIMIZED } else { SW_SHOW });
        let _ = UpdateWindow(h.form);
        let _ = PostMessageW(Some(h.form), WM_APP_SHOWN, WPARAM(0), LPARAM(0));
        let _ = SetFocus(Some(h.list));
    }
}

// ---- Layout ----------------------------------------------------------------------------------------

fn layout() {
    let Some(h) = HANDLES.with(|h| h.get()) else { return };
    if h.list.is_invalid() || h.status.is_invalid() {
        return;
    }
    unsafe {
        let mut client = RECT::default();
        let _ = GetClientRect(h.form, &mut client);
        let (cw, ch) = (client.right - client.left, client.bottom - client.top);

        // Status bar (docked bottom): it sizes itself when it gets WM_SIZE.
        send(h.status, WM_SIZE, 0, 0);
        let mut status_rect = RECT::default();
        let _ = GetClientRect(h.status, &mut status_rect);
        let status_height = status_rect.bottom - status_rect.top;
        layout_status_parts(&h, cw);

        // Filter panel (docked top, 38 px high)
        let panel = scale(38);
        let _ = MoveWindow(h.filter_label, scale(10), scale(12), scale(48), scale(16), true);
        let _ = MoveWindow(h.filter_box, scale(60), scale(9), scale(360), scale(23), true);
        // Anchored top + right: 15 px from the right edge.
        let _ = MoveWindow(h.remove_button, cw - scale(165), scale(6), scale(150), scale(26), true);

        // List (fills the rest)
        let _ = MoveWindow(h.list, 0, panel, cw, (ch - panel - status_height).max(0), true);
    }
}

fn layout_status_parts(h: &Handles, client_width: i32) {
    unsafe {
        if PROGRESS_VISIBLE.with(|p| p.get()) {
            let edges = [client_width - scale(220 + 24), -1];
            send(h.status, SB_SETPARTS, 2, edges.as_ptr() as isize);
            let mut rect = RECT::default();
            send(h.status, SB_GETRECT, 1, &mut rect as *mut _ as isize);
            let _ = MoveWindow(
                h.progress,
                rect.left + 2,
                rect.top + 3,
                (rect.right - rect.left - 4).max(1),
                (rect.bottom - rect.top - 6).max(1),
                true,
            );
        } else {
            let edges = [-1i32];
            send(h.status, SB_SETPARTS, 1, edges.as_ptr() as isize);
        }
    }
}

// ---- Status bar, busy state and progress -----------------------------------------------------------

/// Sets the status bar text without processing messages.
pub fn set_status_label(text: &str) {
    let h = handles();
    let wide_text = wide(text);
    // SBT_NOBORDERS (0x100): a plain label, like a ToolStripStatusLabel.
    send(h.status, SB_SETTEXTW, 0x100, wide_text.as_ptr() as isize);
}

/// Shows a message in the status bar and lets the window repaint (Set-StatusText).
pub fn set_status_text(text: &str) {
    set_status_label(text);
    crate::win::do_events();
}

/// While busy the whole window is disabled, so a second action cannot start in the middle of the first.
pub fn set_busy(busy: bool) {
    let h = handles();
    IS_BUSY.store(busy, Ordering::SeqCst);
    unsafe {
        let _ = EnableWindow(h.form, !busy);
        if busy {
            SetCursor(LoadCursorW(None, IDC_WAIT).ok());
        } else {
            SetCursor(LoadCursorW(None, IDC_ARROW).ok());
            // A disabled window loses the keyboard focus; give it back to the list.
            let focus = GetFocus();
            if focus.is_invalid() {
                let _ = SetFocus(Some(h.list));
            }
        }
    }
    crate::win::do_events();
}

pub fn show_progress(maximum: usize) {
    let h = handles();
    PROGRESS_VISIBLE.with(|p| p.set(true));
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(h.form, &mut client);
    }
    layout_status_parts(&h, client.right - client.left);
    send(h.progress, PBM_SETRANGE32, 0, maximum.max(1) as isize);
    send(h.progress, PBM_SETPOS, 0, 0);
    unsafe {
        let _ = ShowWindow(h.progress, SW_SHOW);
    }
    PROGRESS_POS.with(|p| p.set((0, maximum.max(1))));
}

thread_local! {
    static PROGRESS_POS: Cell<(usize, usize)> = const { Cell::new((0, 1)) };
}

pub fn step_progress() {
    let h = handles();
    let (value, maximum) = PROGRESS_POS.with(|p| p.get());
    if value < maximum {
        PROGRESS_POS.with(|p| p.set((value + 1, maximum)));
        send(h.progress, PBM_SETPOS, value + 1, 0);
    }
    crate::win::do_events();
}

pub fn hide_progress() {
    let h = handles();
    PROGRESS_VISIBLE.with(|p| p.set(false));
    unsafe {
        let _ = ShowWindow(h.progress, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
    }
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(h.form, &mut client);
    }
    layout_status_parts(&h, client.right - client.left);
}

/// Re-enables the window and hides the progress bar (used when an action fails half-way).
pub fn restore_ui() {
    hide_progress();
    if is_busy() {
        set_busy(false);
    }
}

// ---- The list --------------------------------------------------------------------------------------

pub fn filter_text() -> String {
    let h = handles();
    unsafe {
        let length = GetWindowTextLengthW(h.filter_box);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(h.filter_box, &mut buffer);
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }
}

/// What is needed to draw one row.
pub struct RowData {
    pub texts: [String; 14],
    pub color: Option<(u8, u8, u8)>,
    pub checked: bool,
    pub group: Option<usize>,
}

fn colorref(rgb: (u8, u8, u8)) -> u32 {
    (rgb.2 as u32) << 16 | (rgb.1 as u32) << 8 | rgb.0 as u32
}

/// Rebuilds the list control (Items.Clear / Groups.Clear / Items.Add inside BeginUpdate/EndUpdate).
pub fn populate_list(rows: &[RowData], group_headers: &[String]) {
    let h = handles();

    ROW_COLORS.with(|c| *c.borrow_mut() = rows.iter().map(|r| r.color.map(colorref).unwrap_or(u32::MAX)).collect());

    send(h.list, WM_SETREDRAW, 0, 0);
    send(h.list, LVM_DELETEALLITEMS, 0, 0);
    send(h.list, LVM_REMOVEALLGROUPS, 0, 0);
    // Like a WinForms ListView: the group view is only on when there is at least one group.
    send(h.list, LVM_ENABLEGROUPVIEW, if group_headers.is_empty() { 0 } else { 1 }, 0);

    for (index, header) in group_headers.iter().enumerate() {
        let mut text = wide(header);
        let group = LVGROUP {
            cbSize: std::mem::size_of::<LVGROUP>() as u32,
            mask: LVGF_HEADER | LVGF_GROUPID | LVGF_ALIGN,
            pszHeader: PWSTR(text.as_mut_ptr()),
            iGroupId: index as i32,
            uAlign: LVGA_HEADER_LEFT,
            ..Default::default()
        };
        send(h.list, LVM_INSERTGROUP, usize::MAX, lparam_of(&group).0);
    }

    for (index, row) in rows.iter().enumerate() {
        let mut text = wide(&row.texts[0]);
        let mut mask = LVIF_TEXT | LVIF_PARAM;
        let mut group_id = 0;
        if let Some(g) = row.group {
            mask |= LVIF_GROUPID;
            group_id = g as i32;
        }
        let item = LVITEMW {
            mask,
            iItem: index as i32,
            iSubItem: 0,
            pszText: PWSTR(text.as_mut_ptr()),
            lParam: LPARAM(index as isize),
            iGroupId: group_id,
            ..Default::default()
        };
        send(h.list, LVM_INSERTITEMW, 0, lparam_of(&item).0);

        // `$item.Checked = $package.Checked`: the check box is set AFTER the item exists (LVM_SETITEMSTATE),
        // which is the reliable way; a state image given to LVM_INSERTITEM is not.
        if row.checked {
            let state = LVITEMW {
                stateMask: LVIS_STATEIMAGEMASK,
                state: windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(2u32 << 12),
                ..Default::default()
            };
            send(h.list, LVM_SETITEMSTATE, index, lparam_of(&state).0);
        }

        for (column, value) in row.texts.iter().enumerate().skip(1) {
            let mut sub_text = wide(value);
            let sub = LVITEMW {
                iSubItem: column as i32,
                pszText: PWSTR(sub_text.as_mut_ptr()),
                ..Default::default()
            };
            send(h.list, LVM_SETITEMTEXTW, index, lparam_of(&sub).0);
        }
    }

    // A list that has the keyboard focus selects the first row it receives. The list opens with nothing
    // selected: remove the selection and the focus mark of every row (index -1 = all items).
    let none = LVITEMW {
        stateMask: windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
        state: windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(0),
        ..Default::default()
    };
    send(h.list, LVM_SETITEMSTATE, usize::MAX, lparam_of(&none).0);

    send(h.list, WM_SETREDRAW, 1, 0);
    unsafe {
        let _ = InvalidateRect(Some(h.list), None, true);
    }
}

/// Column titles, with ^ / v on the sorted column (Update-ColumnHeaders).
pub fn update_column_headers(sort_column: usize, descending: bool) {
    let h = handles();
    for (index, definition) in COLUMN_DEFINITIONS.iter().enumerate() {
        let mut title = definition.title.to_string();
        if index == sort_column {
            title.push_str(if descending { "  v" } else { "  ^" });
        }
        let mut text = wide(&title);
        let column = LVCOLUMNW { mask: LVCF_TEXT, pszText: PWSTR(text.as_mut_ptr()), ..Default::default() };
        send(h.list, LVM_SETCOLUMNW, index, lparam_of(&column).0);
    }
}

pub fn list_item_count() -> usize {
    send(handles().list, LVM_GETITEMCOUNT, 0, 0).0.max(0) as usize
}

/// Width of each column in 96-dpi pixels, only for the columns that are not at their default width.
pub fn column_widths() -> Vec<Option<i32>> {
    let list = handles().list;
    let dpi = HANDLES.with(|h| h.get()).map(|h| h.dpi).unwrap_or(96) as i64;
    COLUMN_DEFINITIONS
        .iter()
        .enumerate()
        .map(|(index, definition)| {
            let pixels = send(list, LVM_GETCOLUMNWIDTH, index, 0).0 as i64;
            let width = ((pixels * 96 + dpi / 2) / dpi) as i32;
            if width > 0 && (width - definition.width).abs() > 1 { Some(width) } else { None }
        })
        .collect()
}

/// Applies saved column widths (96-dpi pixels; None = leave the default).
pub fn set_column_widths(widths: &[Option<i32>]) {
    let list = handles().list;
    for (index, width) in widths.iter().enumerate().take(COLUMN_DEFINITIONS.len()) {
        if let Some(width) = width {
            send(list, LVM_SETCOLUMNWIDTH, index, scale(*width) as isize);
        }
    }
}

/// Sets the title of the main window.
pub fn set_window_title(title: &str) {
    let text = wide(title);
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(handles().form, PCWSTR(text.as_ptr()));
    }
}

/// Remembers the position and size of the window while it is neither maximized nor minimized. Screen
/// coordinates from GetWindowRect: the coordinates of GetWindowPlacement are relative to the work area, and
/// restoring them as screen coordinates moves the window by the size of the taskbar every time.
fn track_normal_rect(window: HWND) {
    unsafe {
        if IsZoomed(window).as_bool() || IsIconic(window).as_bool() {
            return;
        }
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_ok() {
            NORMAL_RECT.with(|r| r.set(Some(rect)));
        }
    }
}

/// Position, size and maximized state of the window, for the settings file.
pub fn window_state() -> Option<WindowState> {
    let form = handles().form;
    track_normal_rect(form);
    let rect = NORMAL_RECT.with(|r| r.get())?;
    Some(WindowState {
        left: rect.left,
        top: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
        maximized: unsafe { IsZoomed(form).as_bool() },
    })
}

/// Indexes of all selected (highlighted) items, in list order.
pub fn selected_items() -> Vec<usize> {
    let list = handles().list;
    let mut items = Vec::new();
    let mut previous = usize::MAX; // -1: start from the top
    loop {
        let next = send(list, LVM_GETNEXTITEM, previous, LVNI_SELECTED as isize).0;
        if next < 0 {
            break;
        }
        items.push(next as usize);
        previous = next as usize;
    }
    items
}

/// Index of the first selected item (SelectedItems[0]).
pub fn first_selected_item() -> Option<usize> {
    let r = send(handles().list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize).0;
    if r < 0 {
        None
    } else {
        Some(r as usize)
    }
}

// ---- Context menu ----------------------------------------------------------------------------------

fn show_context_menu(lparam: LPARAM) {
    let h = handles();
    unsafe {
        let mut x = (lparam.0 & 0xFFFF) as i16 as i32;
        let mut y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        if lparam.0 == -1 || (x == -1 && y == -1) {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            x = point.x;
            y = point.y;
        }
        let _ = TrackPopupMenu(h.context_menu, TPM_RIGHTBUTTON, x, y, Some(0), h.form, None);
    }
}

// ---- Window procedure ------------------------------------------------------------------------------

fn lo_word(value: usize) -> u16 {
    (value & 0xFFFF) as u16
}
fn hi_word(value: usize) -> u16 {
    ((value >> 16) & 0xFFFF) as u16
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_SIZE => {
            track_normal_rect(window);
            layout();
            LRESULT(0)
        }
        WM_MOVE => {
            track_normal_rect(window);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = lo_word(wparam.0);
            let notification = hi_word(wparam.0);
            if lparam.0 != 0 {
                // From a control.
                if id == ID_FILTER && notification as u32 == EN_CHANGE {
                    app::on_filter_changed();
                } else if id == ID_REMOVE_BUTTON && notification as u32 == BN_CLICKED {
                    app::on_remove_button();
                }
            } else if notification <= 1 {
                // From a menu (0) or an accelerator (1).
                app::on_command(id);
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let header = &*(lparam.0 as *const NMHDR);
            if header.idFrom == ID_LIST as usize || header.hwndFrom == handles().list {
                match header.code {
                    NM_CUSTOMDRAW => return custom_draw(lparam),
                    LVN_ITEMCHANGED => {
                        let n = &*(lparam.0 as *const NMListViewAlias);
                        // The check box state changed (state image index 1 = unchecked, 2 = checked).
                        let changed_state = (n.uChanged.0 & LVIF_STATE.0) != 0;
                        let image_old = (n.uOldState & LVIS_STATEIMAGEMASK.0) >> 12;
                        let image_new = (n.uNewState & LVIS_STATEIMAGEMASK.0) >> 12;
                        if changed_state && n.iItem >= 0 && image_old != image_new && !is_populating() {
                            app::on_item_checked(n.iItem as usize, image_new == 2);
                        }
                        return LRESULT(0);
                    }
                    LVN_COLUMNCLICK => {
                        let n = &*(lparam.0 as *const NMListViewAlias);
                        app::on_column_click(n.iSubItem.max(0) as usize);
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }
            DefWindowProcW(window, message, wparam, lparam)
        }
        WM_CONTEXTMENU => {
            let source = HWND(wparam.0 as *mut _);
            let list = handles().list;
            if source == list || IsChild(list, source).as_bool() {
                show_context_menu(lparam);
                return LRESULT(0);
            }
            DefWindowProcW(window, message, wparam, lparam)
        }
        WM_SETCURSOR => {
            // Form.Cursor = WaitCursor while an operation runs.
            if is_busy() && lo_word(lparam.0 as usize) as u32 == HTCLIENT {
                SetCursor(LoadCursorW(None, IDC_WAIT).ok());
                return LRESULT(1);
            }
            DefWindowProcW(window, message, wparam, lparam)
        }
        WM_APP_SHOWN => {
            app::on_shown();
            LRESULT(0)
        }
        WM_CLOSE => {
            // A removal or export must never be cut in half: the window cannot be closed while an operation runs.
            if is_busy() {
                crate::applog::warn("Close request ignored: an operation is still running.");
                return LRESULT(0);
            }
            app::on_closing();
            let _ = DestroyWindow(window);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

/// NMLISTVIEW, under a local name so the match arms above read clearly.
type NMListViewAlias = NMLISTVIEW;

/// Row colors: the background of an item is its package color (Get-RowColor).
unsafe fn custom_draw(lparam: LPARAM) -> LRESULT {
    const CDDS_PREPAINT: u32 = 0x0000_0001;
    const CDDS_ITEM: u32 = 0x0001_0000;
    const CDDS_ITEMPREPAINT: u32 = CDDS_ITEM | CDDS_PREPAINT;
    const CDRF_NEWFONT: isize = 0x0000_0002;
    const CDRF_NOTIFYITEMDRAW: isize = 0x0000_0020;
    const CDRF_DODEFAULT: isize = 0;

    let draw = &mut *(lparam.0 as *mut NMLVCUSTOMDRAW);
    match draw.nmcd.dwDrawStage.0 {
        CDDS_PREPAINT => LRESULT(CDRF_NOTIFYITEMDRAW),
        CDDS_ITEMPREPAINT => {
            // Group headers are custom-drawn through the same notification (LVCDI_GROUP = 1): leave them alone.
            if draw.dwItemType.0 != 0 {
                return LRESULT(CDRF_DODEFAULT);
            }
            let index = draw.nmcd.dwItemSpec;
            let color = ROW_COLORS.with(|c| c.borrow().get(index).copied());
            match color {
                Some(value) if value != u32::MAX => {
                    draw.clrTextBk = COLORREF(value);
                    LRESULT(CDRF_NEWFONT)
                }
                _ => LRESULT(CDRF_DODEFAULT),
            }
        }
        _ => LRESULT(CDRF_DODEFAULT),
    }
}

// ---- Read-only text window (Help, About) -------------------------------------------------------------

thread_local! {
    static TEXT_WINDOW_OPEN: Cell<bool> = const { Cell::new(false) };
    static TEXT_WINDOW_CHILDREN: Cell<Option<(HWND, HWND)>> = const { Cell::new(None) };
}

fn text_window_layout(window: HWND) {
    let Some((edit, button)) = TEXT_WINDOW_CHILDREN.with(|c| c.get()) else { return };
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(window, &mut client).is_err() {
            return;
        }
        let margin = scale(12);
        let (button_width, button_height) = (scale(90), scale(28));
        let width = client.right - client.left;
        let height = client.bottom - client.top;
        let _ = MoveWindow(edit, margin, margin, (width - 2 * margin).max(10), (height - 3 * margin - button_height).max(10), true);
        let _ = MoveWindow(button, width - margin - button_width, height - margin - button_height, button_width, button_height, true);
    }
}

unsafe extern "system" fn text_window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::Graphics::Gdi::{GetSysColor, GetSysColorBrush, SetBkColor, COLOR_WINDOW, HDC};
    use windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORSTATIC;
    match message {
        WM_SIZE => {
            text_window_layout(window);
            LRESULT(0)
        }
        // OK (button or Enter) and Esc.
        WM_COMMAND if matches!(lo_word(wparam.0), 1 | 2) => {
            let _ = DestroyWindow(window);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(window);
            LRESULT(0)
        }
        WM_DESTROY => {
            TEXT_WINDOW_OPEN.with(|o| o.set(false));
            LRESULT(0)
        }
        // A read-only edit control is gray by default; the text window shows it like normal text.
        WM_CTLCOLORSTATIC => {
            SetBkColor(HDC(wparam.0 as *mut _), COLORREF(GetSysColor(COLOR_WINDOW)));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

/// Shows a text in a window of its own (wider than a message box, resizable, with a scroll bar) and waits
/// until it is closed. The main window is disabled meanwhile. Line breaks in `text` are "\n".
pub fn show_text_window(title: &str, text: &str) {
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::UI::Controls::EM_SETSEL;
    use windows::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, DispatchMessageW, GetMessageW, GetWindowRect, IsDialogMessageW, SetForegroundWindow,
        SetWindowTextW, TranslateMessage, BS_DEFPUSHBUTTON, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, MSG,
        WS_CAPTION, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WS_VSCROLL,
    };
    if TEXT_WINDOW_OPEN.with(|o| o.get()) {
        return;
    }
    let fallback = || crate::win::show_message(text, crate::win::Icon::Information);
    let owner = handles().form;
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return fallback() };
        let instance: HINSTANCE = module.into();
        let class_name = w!("DriverStoreManagerTextWindow");
        let icon = LoadIconW(Some(instance), PCWSTR(ICON_RESOURCE_ID as *const u16)).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(text_window_proc),
            hInstance: instance,
            hIcon: icon,
            hIconSm: icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(16 as *mut _), // COLOR_BTNFACE + 1
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&class); // fails harmlessly when it is already registered

        // 880 x 520 (96-dpi pixels) of client area, kept inside the work area and centered on the main window.
        let style = WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_THICKFRAME.0 | WS_CLIPCHILDREN.0;
        let mut frame = RECT { left: 0, top: 0, right: scale(880), bottom: scale(520) };
        let _ = AdjustWindowRectEx(&mut frame, WINDOW_STYLE(style), false, WINDOW_EX_STYLE(0));
        let (mut width, mut height) = (frame.right - frame.left, frame.bottom - frame.top);
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let mut owner_rect = RECT::default();
        let _ = GetWindowRect(owner, &mut owner_rect);
        if work.right > work.left {
            width = width.min(work.right - work.left - scale(40));
            height = height.min(work.bottom - work.top - scale(40));
        }
        let mut x = owner_rect.left + ((owner_rect.right - owner_rect.left) - width) / 2;
        let mut y = owner_rect.top + ((owner_rect.bottom - owner_rect.top) - height) / 2;
        if work.right > work.left {
            x = x.min(work.right - width).max(work.left);
            y = y.min(work.bottom - height).max(work.top);
        }

        let title_wide = wide(title);
        let Ok(window) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(title_wide.as_ptr()),
            WINDOW_STYLE(style),
            x,
            y,
            width,
            height,
            Some(owner),
            None,
            Some(instance),
            None,
        ) else {
            return fallback();
        };

        let font = message_font();
        let edit_style = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_VSCROLL.0 | ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32;
        let button_style = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32;
        let edit = child(WS_EX_CLIENTEDGE.0, w!("EDIT"), "", edit_style, window, 100, instance, font);
        let button = child(0, w!("BUTTON"), "OK", button_style, window, 1, instance, font); // 1 = IDOK
        let (Ok(edit), Ok(button)) = (edit, button) else {
            let _ = DestroyWindow(window);
            let _ = DeleteObject(font.into());
            return fallback();
        };
        TEXT_WINDOW_CHILDREN.with(|c| c.set(Some((edit, button))));
        let body = wide(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
        let _ = SetWindowTextW(edit, PCWSTR(body.as_ptr()));
        send(edit, EM_SETSEL, 0, 0); // nothing selected
        text_window_layout(window);

        TEXT_WINDOW_OPEN.with(|o| o.set(true));
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetFocus(Some(button));

        // Own message loop until the window is destroyed (the same thing a message box does).
        let mut message = MSG::default();
        while TEXT_WINDOW_OPEN.with(|o| o.get()) {
            let got = GetMessageW(&mut message, None, 0, 0);
            if got.0 <= 0 {
                if got.0 == 0 {
                    PostQuitMessage(message.wParam.0 as i32); // the program is quitting: pass it on
                }
                break;
            }
            if !IsDialogMessageW(window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        TEXT_WINDOW_CHILDREN.with(|c| c.set(None));
        TEXT_WINDOW_OPEN.with(|o| o.set(false));
        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        let _ = DeleteObject(font.into());
    }
}

/// Closes the window (Form.Close()): goes through WM_CLOSE so a running operation can veto it.
pub fn close_form() {
    unsafe {
        let _ = PostMessageW(Some(handles().form), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}
