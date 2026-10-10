//! The commands behind the menu items, the way they read and change the Driver Store, and the messages they
//! show. The pure rules live in `model`; the window itself is in `ui`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};

use crate::applog;
use crate::backup::{self, ManifestPackage};
use crate::drvstore;
use crate::export;
use crate::format::{format_date, format_package_list, format_size, output_tail};
use crate::fsops::{self, clean_io};
use crate::model::{
    self, build_view, is_auto_selectable, is_package_listed, is_unused_selectable, new_device_map, new_driver_records,
    new_offline_driver_records, row_color, row_texts, summary_text, visible_packages, Driver, RawPackage, ViewModel, APP_FILE_NAME, APP_NAME,
    APP_VERSION, COLUMN_DEFINITIONS, DEFAULT_GROUP_MODE, GROUP_MODES,
};
use crate::native::{self, ImageTarget};
use crate::pnputil;
use crate::proc;
use crate::settings::{self, Settings};
use crate::ui::{self, *};
use crate::win::{self, confirm_action, show_message, Icon};

// ---- State -----------------------------------------------------------------------------------------

thread_local! {
    /// All packages.
    static DRIVERS: RefCell<Vec<Driver>> = const { RefCell::new(Vec::new()) };
    /// What the list shows right now: row -> package index, row -> group.
    static VIEW: RefCell<ViewModel> = RefCell::new(ViewModel::default());
    static SORT_COLUMN: Cell<usize> = const { Cell::new(0) };
    static SORT_DESCENDING: Cell<bool> = const { Cell::new(false) };
    static GROUP_MODE: Cell<usize> = const { Cell::new(DEFAULT_GROUP_MODE) };
    /// The Windows being managed: the running one, or an offline image.
    static IMAGE: RefCell<ImageTarget> = const { RefCell::new(ImageTarget::Online) };
    /// Where backups of an offline image go (asked once per opened image).
    static OFFLINE_BACKUP_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Protection keys of the packages the user protected (model::protection_key).
    static PROTECTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn image() -> ImageTarget {
    IMAGE.with(|i| i.borrow().clone())
}

fn set_image(target: ImageTarget) {
    let title = match &target {
        ImageTarget::Online => format!("{APP_NAME} {APP_VERSION} - third-party driver packages"),
        ImageTarget::Offline(path) => format!("{APP_NAME} {APP_VERSION} - OFFLINE IMAGE {path}"),
    };
    IMAGE.with(|i| *i.borrow_mut() = target);
    OFFLINE_BACKUP_ROOT.with(|r| *r.borrow_mut() = None);
    ui::set_window_title(&title);
}

fn drivers_snapshot() -> Vec<Driver> {
    DRIVERS.with(|d| d.borrow().clone())
}

/// "True" / "False", the spelling used in the log.
fn ps_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

fn pairs(list: &[&Driver]) -> Vec<(String, String)> {
    list.iter().map(|p| (p.published_name.clone(), p.original_inf.clone())).collect()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// The settings file, next to the program.
fn settings_file() -> PathBuf {
    applog::app_dir().join(format!("{APP_FILE_NAME}.ini"))
}

/// Loads the settings file (before the window is created, so the window can use its saved position).
pub fn load_settings() -> Settings {
    let (settings, warning) = settings::load(&settings_file(), GROUP_MODES.len(), COLUMN_DEFINITIONS.len());
    if let Some(text) = warning {
        applog::warn(&text);
    }
    settings
}

/// Applies the settings to the menus, the sort order and the grouping (the window exists by now).
pub fn apply_settings(settings: &Settings) {
    GROUP_MODE.with(|g| g.set(settings.group_mode));
    SORT_COLUMN.with(|c| c.set(settings.sort_column));
    SORT_DESCENDING.with(|d| d.set(settings.sort_descending));
    for index in 0..GROUP_MODES.len() {
        ui::set_menu_checked(ID_GROUP_BASE + index as u16, index == settings.group_mode);
    }
    ui::set_menu_checked(ID_OPT_BACKUP, settings.back_up);
    ui::set_menu_checked(ID_OPT_INCLUDE_BOOT, settings.include_boot_critical);
    ui::set_menu_checked(ID_OLD_ONLY, settings.old_only);
    ui::set_menu_checked(ID_DISCONNECTED_ONLY, settings.disconnected_only);
    ui::set_menu_checked(ID_PROBLEM_ONLY, settings.problem_only);
    ui::set_column_widths(&settings.column_widths);
    PROTECTED.with(|p| *p.borrow_mut() = settings.protected.clone());
}

/// Called when the window is closing: remembers the options and the window position.
pub fn on_closing() {
    let current = Settings {
        back_up: ui::menu_checked(ID_OPT_BACKUP),
        include_boot_critical: ui::menu_checked(ID_OPT_INCLUDE_BOOT),
        old_only: ui::menu_checked(ID_OLD_ONLY),
        disconnected_only: ui::menu_checked(ID_DISCONNECTED_ONLY),
        problem_only: ui::menu_checked(ID_PROBLEM_ONLY),
        column_widths: ui::column_widths(),
        group_mode: GROUP_MODE.with(|g| g.get()),
        sort_column: SORT_COLUMN.with(|c| c.get()),
        sort_descending: SORT_DESCENDING.with(|d| d.get()),
        window: ui::window_state(),
        protected: PROTECTED.with(|p| p.borrow().clone()),
    };
    if let Err(error) = settings::save(&settings_file(), &current) {
        applog::warn(&format!("The settings were not saved: {error}"));
    }
}

// ---- Uniform error handling --------------------------------------------------------------------------

/// Runs one user action with uniform error handling: the action is logged, and any error is written to
/// the log and explained in a message box instead of crashing the window.
pub fn ui_action<F: FnOnce() -> Result<()>>(name: &str, quiet: bool, body: F) {
    if !quiet {
        applog::info(&format!("Action: {name}"));
    }
    let outcome = catch_unwind(AssertUnwindSafe(body));
    let result = match outcome {
        Ok(result) => result,
        Err(panic) => {
            let text = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unexpected internal error".to_string());
            Err(anyhow!("{text}"))
        }
    };
    if let Err(error) = result {
        applog::error(&format!("Action '{name}' failed: {error}"));
        applog::error(&format!("Error details: {error:?}"));
        ui::restore_ui();
        show_message(
            &format!(
                "'{name}' failed:\n\n{error}\n\nDetails are in the log:\n{}\n\nIf the list may be out of date, use File > Refresh (F5).",
                path_text(applog::log_file())
            ),
            Icon::Error,
        );
    }
}

// ---- Events coming from the window ------------------------------------------------------------------

pub fn on_shown() {
    ui_action("Load driver packages", false, update_inventory);
}

pub fn on_filter_changed() {
    ui_action("Filter", true, || {
        update_view();
        Ok(())
    });
}

pub fn on_remove_button() {
    ui_action("Remove checked driver packages", false, remove_checked_packages);
}

pub fn on_item_checked(index: usize, checked: bool) {
    ui_action("Check item", true, || {
        if ui::is_populating() {
            return Ok(());
        }
        let driver = VIEW.with(|v| v.borrow().rows.get(index).map(|r| r.driver));
        if let Some(driver) = driver {
            DRIVERS.with(|d| {
                if let Some(p) = d.borrow_mut().get_mut(driver) {
                    p.checked = checked;
                }
            });
            update_summary();
        }
        Ok(())
    });
}

pub fn on_column_click(column: usize) {
    ui_action("Sort", true, || {
        if column == SORT_COLUMN.with(|c| c.get()) {
            SORT_DESCENDING.with(|d| d.set(!d.get()));
        } else {
            SORT_COLUMN.with(|c| c.set(column));
            SORT_DESCENDING.with(|d| d.set(false));
        }
        update_view();
        Ok(())
    });
}

pub fn on_command(id: u16) {
    match id {
        ID_REFRESH => ui_action("Refresh", false, update_inventory),
        ID_EXPORT_LIST => ui_action("Export list", false, export_list),
        ID_OPEN_LOG => ui_action("Open log folder", false, || open_folder(applog::log_dir())),
        ID_OPEN_BACKUP => ui_action("Open backup folder", false, || {
            fsops::ensure_plain_dir(applog::backup_root())?;
            open_folder(applog::backup_root())
        }),
        ID_EXIT => ui::close_form(),

        ID_ADD => ui_action("Add driver package", false, || add_driver_packages(false)),
        ID_ADD_INSTALL => ui_action("Add and install driver package", false, || add_driver_packages(true)),
        ID_EXPORT_CHECKED => ui_action("Export checked driver packages", false, || export_driver_packages(false)),
        ID_EXPORT_ALL => ui_action("Export all driver packages", false, || export_driver_packages(true)),
        ID_OPEN_OFFLINE => ui_action("Open offline Windows image", false, open_offline_image),
        ID_BACK_ONLINE => ui_action("Return to the running Windows", false, back_to_online),
        ID_RESTORE => ui_action("Restore backup", false, restore_backup),
        ID_VERIFY_BACKUP => ui_action("Verify backup", false, verify_backup),
        ID_REMOVE => ui_action("Remove checked driver packages", false, remove_checked_packages),

        ID_CHECK_OLD_UNUSED => {
            ui_action("Check old packages (unused only - safe)", false, || set_checked_by_rule(false))
        }
        ID_CHECK_OLD_IN_USE => {
            ui_action("Check old packages (including in use)", false, || set_checked_by_rule(true))
        }
        ID_CHECK_UNUSED => ui_action("Check unused packages", false, set_checked_unused),
        ID_INVERT => ui_action("Invert checks of everything shown", false, invert_shown),
        ID_CHECK_SHOWN => ui_action("Check everything shown", false, || set_items_checked(all_rows(), true)),
        ID_UNCHECK_SHOWN => ui_action("Uncheck everything shown", false, || set_items_checked(all_rows(), false)),
        ID_UNCHECK_ALL => ui_action("Uncheck all", false, set_all_unchecked),

        id if id >= ID_GROUP_BASE && (id - ID_GROUP_BASE) < GROUP_MODES.len() as u16 => {
            ui_action("Group by", true, || set_group_mode((id - ID_GROUP_BASE) as usize));
        }
        ID_OLD_ONLY => {
            // CheckOnClick: the item is toggled before the handler runs.
            ui::toggle_menu_checked(ID_OLD_ONLY);
            ui_action("Show only old packages", true, || {
                applog::info(&format!("Show only old packages: {}", ps_bool(ui::menu_checked(ID_OLD_ONLY))));
                update_view();
                Ok(())
            });
        }
        ID_DISCONNECTED_ONLY => {
            ui::toggle_menu_checked(ID_DISCONNECTED_ONLY);
            ui_action("Show only packages used only by disconnected devices", true, || {
                applog::info(&format!(
                    "Show only packages used only by disconnected devices: {}",
                    ps_bool(ui::menu_checked(ID_DISCONNECTED_ONLY))
                ));
                update_view();
                Ok(())
            });
        }
        ID_PROBLEM_ONLY => {
            ui::toggle_menu_checked(ID_PROBLEM_ONLY);
            ui_action("Show only packages used by devices with a problem", true, || {
                applog::info(&format!(
                    "Show only packages used by devices with a problem: {}",
                    ps_bool(ui::menu_checked(ID_PROBLEM_ONLY))
                ));
                update_view();
                Ok(())
            });
        }
        ID_OPT_BACKUP => {
            let value = ui::toggle_menu_checked(ID_OPT_BACKUP);
            applog::info(&format!("Option 'Back up packages before removing' is now: {}", ps_bool(value)));
        }
        ID_OPT_INCLUDE_BOOT => {
            ui::toggle_menu_checked(ID_OPT_INCLUDE_BOOT);
            ui_action("Option: include boot-critical packages in \"Check old packages\"", true, || {
                show_boot_critical_effect();
                Ok(())
            });
        }
        ID_HELP_HOW => ui_action("How it works", false, || {
            show_how_it_works();
            Ok(())
        }),
        ID_HELP_ABOUT => ui_action("About", false, || {
            show_about();
            Ok(())
        }),

        ID_CTX_CHECK_GROUP => ui_action("Check group", false, || check_group(true)),
        ID_CTX_UNCHECK_GROUP => ui_action("Uncheck group", false, || check_group(false)),
        ID_CTX_REMOVE_SELECTED => ui_action("Remove selected driver packages", false, remove_selected_packages),
        ID_CTX_PROTECT => ui_action("Protect or unprotect packages", false, toggle_protection),
        ID_CTX_REMOVE_DEVICES => ui_action("Remove disconnected devices", false, remove_disconnected_devices),
        ID_CTX_EXPORT_SELECTED => ui_action("Export selected driver packages", false, export_selected_packages),
        ID_CTX_DEVICE_PROPS => ui_action("Open device properties", false, open_device_properties),
        ID_CTX_OPEN_FOLDER => ui_action("Open package folder", false, open_package_folder),
        ID_CTX_COPY_PATH => ui_action("Copy package folder path", false, copy_package_folder_path),
        ID_CTX_COPY_CELL => ui_action("Copy cell text", false, copy_cell_text),
        ID_CTX_COPY_ROWS => ui_action("Copy selected rows", false, copy_selected_rows),
        ID_FOCUS_FILTER => ui::focus_filter(),
        _ => {}
    }
}

// ---- List view: filter, sort, group, colors, summary -----------------------------------------------------

/// Status bar summary.
pub fn update_summary() {
    let text = DRIVERS.with(|d| summary_text(&d.borrow(), ui::list_item_count()));
    ui::set_status_label(&text);
}

/// Rebuilds the list from the packages using the current filter, sort order and grouping.
pub fn update_view() {
    let (rows, model) = DRIVERS.with(|d| {
        let drivers = d.borrow();
        let visible = visible_packages(
            &drivers,
            &ui::filter_text(),
            ui::menu_checked(ID_OLD_ONLY),
            ui::menu_checked(ID_DISCONNECTED_ONLY),
            ui::menu_checked(ID_PROBLEM_ONLY),
            SORT_COLUMN.with(|c| c.get()),
            SORT_DESCENDING.with(|s| s.get()),
        );
        let group_by = GROUP_MODES[GROUP_MODE.with(|g| g.get())].1;
        let view = build_view(&drivers, &visible, group_by);
        let rows: Vec<RowData> = view
            .rows
            .iter()
            .map(|row| {
                let package = &drivers[row.driver];
                RowData {
                    texts: row_texts(package),
                    color: row_color(package),
                    checked: package.checked,
                    group: row.group,
                }
            })
            .collect();
        (rows, view)
    });
    let headers = model.group_headers.clone();
    VIEW.with(|v| *v.borrow_mut() = model);

    ui::set_populating(true);
    ui::populate_list(&rows, &headers);
    ui::update_column_headers(SORT_COLUMN.with(|c| c.get()), SORT_DESCENDING.with(|s| s.get()));
    ui::set_populating(false);
    update_summary();
}

// ---- Inventory -------------------------------------------------------------------------------------------

/// Reads the devices from Windows and builds the device map.
fn get_device_map() -> Result<HashMap<String, Vec<model::DeviceRef>>> {
    applog::info("Reading devices (Configuration Manager) to find which devices use each package.");
    let devices = native::get_devices()?;
    applog::info(&format!("Found {} devices that use a driver package.", devices.len()));
    Ok(new_device_map(&devices))
}

fn raw_packages(target: &ImageTarget, drivers: Vec<native::WindowsDriver>) -> Result<Vec<RawPackage>> {
    let mut raw = Vec::with_capacity(drivers.len());
    for mut d in drivers {
        // An offline image can report paths with the drive letter the image had when it was running.
        if let ImageTarget::Offline(root) = target {
            if !Path::new(&d.folder).exists() {
                if let (Some(file), Some(folder)) =
                    (native::rebase_to_image(&d.original_file_name, root), native::rebase_to_image(&d.folder, root))
                {
                    if Path::new(&folder).exists() {
                        d.original_file_name = file;
                        d.folder = folder;
                    }
                }
            }
        }
        let number = model::parse_package_number(&d.driver).map_err(|e| anyhow!("{e}"))?;
        // A folder that cannot be read completely does not stop the list: the size is shown as a minimum.
        let mut problems = Vec::new();
        let scan = fsops::scan_folder(Path::new(&d.folder), &mut problems);
        for problem in problems.iter().take(5) {
            applog::warn(&format!("{}: size is incomplete - {problem}", d.driver));
        }
        raw.push(RawPackage {
            published_name: d.driver,
            number,
            original_inf: d.original_inf,
            folder: d.folder,
            extension_id: d.extension_id,
            provider: d.provider,
            class: d.class,
            version: d.version,
            date: d.date,
            boot_critical: d.boot_critical,
            signature: d.signature,
            signer: d.signer,
            install_date: d.install_date,
            size_bytes: scan.bytes,
            size_exact: scan.complete,
            files: scan.files,
        });
        proc::pump_throttled();
    }
    Ok(raw)
}

/// Reads the Driver Store (and the devices, for the running Windows) again and refreshes the list.
pub fn update_inventory() -> Result<()> {
    let target = image();
    ui::set_busy(true);
    ui::set_status_text("Reading driver packages (this can take a few seconds)...");

    applog::info(&format!(
        "Reading driver packages of {} from the Driver Store (drvstore.dll). Only third-party packages (oemNN.inf) are listed.",
        target.describe()
    ));
    let raw = raw_packages(&target, drvstore::get_windows_drivers(&target)?)?;
    applog::info(&format!("Found {} packages.", raw.len()));

    let mut drivers = if target.is_offline() {
        // An offline image has no devices: nothing is asked, and "in use" is unknown.
        new_offline_driver_records(&raw)
    } else {
        ui::set_status_text("Reading devices...");
        let device_map = get_device_map()?;
        new_driver_records(&raw, &device_map)
    };

    PROTECTED.with(|p| {
        let keys = p.borrow();
        for package in drivers.iter_mut() {
            package.protected = keys.contains(&model::protection_key(package));
        }
    });
    let old: Vec<&Driver> = drivers.iter().filter(|d| d.is_old).collect();
    let review: Vec<&Driver> = drivers.iter().filter(|d| d.status == model::Status::Review).collect();
    if target.is_offline() {
        applog::info(&format!(
            "Loaded {} packages: {} old, {} to review (device usage is unknown for an offline image).",
            drivers.len(),
            old.len(),
            review.len()
        ));
    } else {
        applog::info(&format!(
            "Loaded {} packages: {} old ({} unused, {} in use), {} to review.",
            drivers.len(),
            old.len(),
            old.iter().filter(|d| !d.in_use).count(),
            old.iter().filter(|d| d.in_use).count(),
            review.len()
        ));
    }
    for package in old.iter().chain(review.iter()) {
        applog::info(&format!(
            "  {}  {} v{} ({})  used by: {}  [{}]",
            package.published_name,
            package.original_inf,
            package.version,
            format_date(package.date),
            package.device_text,
            package.status_text
        ));
    }
    DRIVERS.with(|d| *d.borrow_mut() = drivers);

    ui::set_busy(false);
    update_view();
    Ok(())
}

// ---- File > Open offline Windows image... ----------------------------------------------------------------

fn open_offline_image() -> Result<()> {
    let Some(mut root) = win::browse_for_folder(
        ui::form(),
        "Select the root of the offline Windows image: the drive or folder that contains the Windows folder (for example D:\\). The image must not be the Windows that is running.",
    ) else {
        applog::info("Opening an offline image was cancelled by the user.");
        return Ok(());
    };
    // Picking the Windows folder itself is accepted: the root is its parent.
    if root.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("windows")).unwrap_or(false) {
        if let Some(parent) = root.parent() {
            root = parent.to_path_buf();
        }
    }
    // The Windows that is running is not an offline image.
    if let Some(running_root) = win::windows_directory().parent() {
        if fsops::same_folder(&path_text(running_root), &path_text(&root)) {
            show_message(
                &format!("{} is the Windows that is running, not an offline image. Its drivers are already shown when you use File > Return to the running Windows.", path_text(&root)),
                Icon::Information,
            );
            return Ok(());
        }
    }
    if !root.join("Windows").join("System32").join("DriverStore").exists() {
        show_message(
            &format!("This is not the root of a Windows image (there is no Windows\\System32\\DriverStore in it):\n{}", path_text(&root)),
            Icon::Warning,
        );
        return Ok(());
    }
    let previous = image();
    set_image(ImageTarget::Offline(path_text(&root)));
    applog::info(&format!("Managing {}.", image().describe()));
    if let Err(error) = update_inventory() {
        // The image could not be read: go back to the running Windows.
        applog::error(&format!("The offline image could not be opened: {error}"));
        set_image(previous);
        let _ = update_inventory();
        return Err(error);
    }
    Ok(())
}

fn back_to_online() -> Result<()> {
    if !image().is_offline() {
        show_message("The running Windows is already being managed.", Icon::Information);
        return Ok(());
    }
    set_image(ImageTarget::Online);
    applog::info("Managing the running Windows again.");
    update_inventory()
}

/// The folder for the backups of an offline image, asked once per opened image. Ok(None) = cancelled.
fn offline_backup_root() -> Result<Option<PathBuf>> {
    if let Some(path) = OFFLINE_BACKUP_ROOT.with(|r| r.borrow().clone()) {
        return Ok(Some(path));
    }
    let Some(folder) = win::browse_for_folder(
        ui::form(),
        "Select the folder where the backups of this image are saved. Use a folder on a disk that is kept after a restart: in Windows PE the X: drive is a RAM disk and its content is lost.",
    ) else {
        return Ok(None);
    };
    fsops::ensure_plain_dir(&folder)?;
    OFFLINE_BACKUP_ROOT.with(|r| *r.borrow_mut() = Some(folder.clone()));
    Ok(Some(folder))
}

// ---- Selection commands ------------------------------------------------------------------------------------

/// Select > Check old packages ...: checks the packages matching the "Check old packages" rule
/// (superseded or duplicate packages; never ntprint.inf).
fn set_checked_by_rule(include_in_use: bool) -> Result<()> {
    let include_boot = ui::menu_checked(ID_OPT_INCLUDE_BOOT);
    let count = DRIVERS.with(|d| {
        let mut drivers = d.borrow_mut();
        for package in drivers.iter_mut() {
            package.checked = is_auto_selectable(package, include_in_use, include_boot);
        }
        drivers.iter().filter(|p| p.checked).count()
    });
    applog::info(&format!(
        "Checked {} old package(s) (in-use packages {}, boot-critical packages {}).",
        count,
        if include_in_use { "included" } else { "excluded" },
        if include_boot { "included" } else { "excluded" }
    ));
    update_view();
    Ok(())
}

/// Select > Check unused packages: checks every package that no device uses, whatever its age.
fn set_checked_unused() -> Result<()> {
    if image().is_offline() {
        show_message("Not available for an offline image: there are no devices, so it is not known which packages are unused.", Icon::Information);
        return Ok(());
    }
    let include_boot = ui::menu_checked(ID_OPT_INCLUDE_BOOT);
    let count = DRIVERS.with(|d| {
        let mut drivers = d.borrow_mut();
        for package in drivers.iter_mut() {
            package.checked = is_unused_selectable(package, include_boot);
        }
        drivers.iter().filter(|p| p.checked).count()
    });
    applog::info(&format!(
        "Checked {count} unused package(s) (no device uses them; boot-critical packages {}).",
        if include_boot { "included" } else { "excluded" }
    ));
    update_view();
    if count == 0 {
        ui::set_status_text("No unused package was found.");
    }
    Ok(())
}

/// Select > Invert checks of everything shown: what is checked becomes unchecked and the other way round.
/// Packages hidden by the filter are not touched.
fn invert_shown() -> Result<()> {
    let drivers: Vec<usize> = VIEW.with(|v| v.borrow().rows.iter().map(|r| r.driver).collect());
    DRIVERS.with(|d| {
        let mut list = d.borrow_mut();
        for index in drivers {
            if let Some(p) = list.get_mut(index) {
                p.checked = !p.checked && !p.protected;
            }
        }
    });
    update_view();
    Ok(())
}

/// Rows (indexes of the list items) of everything shown.
fn all_rows() -> Vec<usize> {
    VIEW.with(|v| (0..v.borrow().rows.len()).collect())
}

/// Checks or unchecks a set of list items (everything shown, or one group).
fn set_items_checked(rows: Vec<usize>, value: bool) -> Result<()> {
    let drivers: Vec<usize> = VIEW.with(|v| {
        let view = v.borrow();
        rows.iter().filter_map(|&r| view.rows.get(r).map(|row| row.driver)).collect()
    });
    DRIVERS.with(|d| {
        let mut list = d.borrow_mut();
        for index in drivers {
            if let Some(p) = list.get_mut(index) {
                // A protected package is never checked in bulk.
                p.checked = value && !p.protected;
            }
        }
    });
    update_view();
    Ok(())
}

/// Select > Uncheck all: unchecks every package, including those hidden by the filter.
fn set_all_unchecked() -> Result<()> {
    DRIVERS.with(|d| {
        for package in d.borrow_mut().iter_mut() {
            package.checked = false;
        }
    });
    update_view();
    Ok(())
}

/// Right-click > Check / Uncheck all in this group.
fn check_group(value: bool) -> Result<()> {
    let Some(selected) = ui::first_selected_item() else { return Ok(()) };
    let rows: Vec<usize> = VIEW.with(|v| {
        let view = v.borrow();
        match view.rows.get(selected).and_then(|r| r.group) {
            Some(group) => view.rows.iter().enumerate().filter(|(_, r)| r.group == Some(group)).map(|(i, _)| i).collect(),
            None => (0..view.rows.len()).collect(),
        }
    });
    set_items_checked(rows, value)
}

fn selected_package() -> Option<Driver> {
    let selected = ui::first_selected_item()?;
    let index = VIEW.with(|v| v.borrow().rows.get(selected).map(|r| r.driver))?;
    DRIVERS.with(|d| d.borrow().get(index).cloned())
}

/// The packages of all selected (highlighted) rows, as opposed to the checked ones.
fn selected_packages() -> Vec<Driver> {
    let rows = ui::selected_items();
    let indexes: Vec<usize> = VIEW.with(|v| {
        let view = v.borrow();
        rows.iter().filter_map(|&r| view.rows.get(r).map(|row| row.driver)).collect()
    });
    DRIVERS.with(|d| {
        let list = d.borrow();
        indexes.iter().filter_map(|&i| list.get(i).cloned()).collect()
    })
}

/// Right-click > Open device properties: Device Manager for the first device bound to the package.
fn open_device_properties() -> Result<()> {
    let Some(package) = selected_package() else { return Ok(()) };
    match package.device_ids.first() {
        Some(id) => win::open_device_properties(id),
        None => {
            show_message("No device uses this package, so there are no device properties to show.", Icon::Information);
            Ok(())
        }
    }
}

fn open_package_folder() -> Result<()> {
    let Some(package) = selected_package() else { return Ok(()) };
    let file = PathBuf::from(&package.folder).join(&package.original_inf);
    win::start_explorer(&format!("/select,\"{}\"", path_text(&file)))
}

fn copy_package_folder_path() -> Result<()> {
    let Some(package) = selected_package() else { return Ok(()) };
    win::set_clipboard_text(ui::form(), &package.folder)
}

/// Right-click > Copy cell text: the text of the cell that was clicked, exactly as the list shows it.
fn copy_cell_text() -> Result<()> {
    let Some((row, column)) = ui::context_cell() else { return Ok(()) };
    let driver = VIEW.with(|v| v.borrow().rows.get(row).map(|r| r.driver));
    let texts = driver.and_then(|index| DRIVERS.with(|d| d.borrow().get(index).map(row_texts)));
    match texts.as_ref().and_then(|t| t.get(column)) {
        Some(text) => win::set_clipboard_text(ui::form(), text),
        None => Ok(()),
    }
}

/// Right-click > Copy selected rows: the highlighted rows as tab-separated text, with the column titles.
fn copy_selected_rows() -> Result<()> {
    let selected = selected_packages();
    if selected.is_empty() {
        show_message("No rows are selected. Click a row (or several, with Ctrl or Shift) first.", Icon::Information);
        return Ok(());
    }
    let rows: Vec<&Driver> = selected.iter().collect();
    win::set_clipboard_text(ui::form(), &export::tsv_text(&rows))
}

/// Options > Include boot-critical packages in the automatic selections: says what the option does right now.
fn show_boot_critical_effect() {
    let state = if ui::menu_checked(ID_OPT_INCLUDE_BOOT) { "included in" } else { "excluded from" };
    let affected = DRIVERS.with(|d| d.borrow().iter().filter(|p| p.is_old && p.boot_critical).count());
    applog::info(&format!(
        "Option changed: boot-critical packages are now {state} 'Check old packages'. Old packages that are boot-critical: {affected}."
    ));
    if affected == 0 {
        ui::set_status_text(&format!(
            "Boot-critical packages are now {state} 'Check old packages'. This changes nothing right now: no old package is flagged boot-critical by Windows."
        ));
    } else {
        ui::set_status_text(&format!(
            "Boot-critical packages are now {state} 'Check old packages' ({affected} old package(s) affected). Use Select > Check old packages again to apply."
        ));
    }
}

/// Sets the grouping and ticks the matching View > Group by menu item.
fn set_group_mode(index: usize) -> Result<()> {
    GROUP_MODE.with(|g| g.set(index));
    for i in 0..GROUP_MODES.len() {
        ui::set_menu_checked(ID_GROUP_BASE + i as u16, i == index);
    }
    applog::info(&format!("Grouping changed to: {}", GROUP_MODES[index].0));
    update_view();
    Ok(())
}

// ---- File > Export list... -------------------------------------------------------------------------------

fn export_list() -> Result<()> {
    let all = drivers_snapshot();
    let shown: Vec<&Driver> = VIEW.with(|v| v.borrow().rows.iter().map(|r| &all[r.driver]).collect());
    if shown.is_empty() {
        show_message("There is nothing to export: the list is empty.", Icon::Information);
        return Ok(());
    }

    let chosen = win::save_file_dialog(
        ui::form(),
        &format!("Export the {} package(s) currently shown", shown.len()),
        &[("CSV - opens in Excel (*.csv)", "*.csv"), ("JSON - for scripts and other tools (*.json)", "*.json")],
        "csv",
        // The moment of this export, not of program start.
        &format!("{APP_FILE_NAME}_list_{}", applog::file_stamp()),
    )?;
    let Some(path) = chosen else {
        applog::info("Export cancelled by the user.");
        return Ok(());
    };

    let is_json = path.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case("json")).unwrap_or(false);
    let bytes = if is_json { export::json_bytes(&shown) } else { export::csv_bytes(&shown, &crate::culture::list_separator()) };
    fs::write(&path, bytes).map_err(|e| anyhow!("Could not write '{}': {}", path_text(&path), clean_io(&e)))?;

    applog::info(&format!("Exported {} package(s) to {}", shown.len(), path_text(&path)));
    show_message(&format!("Exported {} package(s) to:\n{}", shown.len(), path_text(&path)), Icon::Information);
    Ok(())
}

// ---- Drivers > Add driver package... ---------------------------------------------------------------------

/// Add driver package(s) to an OFFLINE image, one DISM call per .inf file.
fn add_driver_packages_offline(folder: PathBuf) -> Result<()> {
    let target = image();
    let folder_text = path_text(&folder);
    let infs = fsops::list_inf_files(&folder).map_err(|e| anyhow!("Could not search '{}': {}", folder_text, clean_io(&e)))?;
    if infs.is_empty() {
        applog::warn(&format!("No .inf files found in {folder_text}"));
        show_message(&format!("No .inf files were found in:\n{folder_text}"), Icon::Warning);
        return Ok(());
    }
    if !confirm_action(
        &format!(
            "Add {} .inf file(s) from:\n{folder_text}\n\nto {}?\n\nThey are added to the image only: an offline image has no devices to install them on. An unsigned driver is refused.\n\nContinue?",
            infs.len(),
            target.describe()
        ),
        Icon::Question,
    ) {
        applog::info("Adding a driver package was cancelled by the user (confirmation).");
        return Ok(());
    }
    applog::info(&format!("Adding {} .inf file(s) from {folder_text} to {}.", infs.len(), target.describe()));
    ui::set_busy(true);
    ui::set_status_text("Adding driver package(s) to the image...");
    let results = native::add_drivers_offline(&target, &infs);
    ui::set_busy(false);
    let results = results?;
    let added = results.iter().filter(|(_, r)| r.is_ok()).count();
    let failures: Vec<String> =
        results.iter().filter_map(|(p, r)| r.as_ref().err().map(|e| format!("{}: {e}", p.display()))).collect();
    for failure in &failures {
        applog::error(&format!("Could not add {failure}"));
    }
    applog::info(&format!("Added {added} of {} .inf file(s).", results.len()));
    let mut summary = format!("Done: {added} added, {} failed.", failures.len());
    if !failures.is_empty() {
        summary.push_str(&format!("\n\n{}", failures.iter().take(8).cloned().collect::<Vec<_>>().join("\n")));
    }
    show_message(&summary, if failures.is_empty() { Icon::Information } else { Icon::Warning });
    update_inventory()
}

fn add_driver_packages(install: bool) -> Result<()> {
    let offline = image().is_offline();
    if offline && install {
        show_message("'Add and install' is only for the running Windows: an offline image has no devices to install on. Use 'Add driver package...' instead.", Icon::Information);
        return Ok(());
    }
    let Some((folder, only_newer)) = win::browse_for_folder_with_option(
        ui::form(),
        "Select the folder that contains the driver package(s) (.inf files). Subfolders are searched too.",
        "Add only newer packages",
    ) else {
        applog::info("Adding a driver package was cancelled by the user (folder dialog).");
        return Ok(());
    };
    if only_newer {
        return add_newer_driver_packages(install, folder);
    }
    if offline {
        return add_driver_packages_offline(folder);
    }
    let folder_text = path_text(&folder);

    let inf_count = fsops::count_inf_files(&folder)
        .map_err(|e| anyhow!("Could not search '{}': {}", folder_text, clean_io(&e)))?;
    if inf_count == 0 {
        applog::warn(&format!("No .inf files found in {folder_text}"));
        show_message(&format!("No .inf files were found in:\n{folder_text}"), Icon::Warning);
        return Ok(());
    }

    let mut question = format!("Add {inf_count} .inf file(s) from:\n{folder_text}\n\n");
    if install {
        question.push_str("They will be added to the Driver Store AND installed on matching devices. Windows installs a package on a device only if it is the best match for it.");
    } else {
        question.push_str("They will be added to the Driver Store only (no device will be changed).");
    }
    if !confirm_action(&format!("{question}\n\nContinue?"), Icon::Question) {
        applog::info("Adding a driver package was cancelled by the user (confirmation).");
        return Ok(());
    }

    applog::info(&format!("Adding {inf_count} .inf file(s) from {folder_text} (install on devices: {}).", ps_bool(install)));
    let mut arguments = vec!["/add-driver".to_string(), path_text(&folder.join("*.inf")), "/subdirs".to_string()];
    if install {
        arguments.push("/install".to_string());
    }

    ui::set_busy(true);
    ui::set_status_text("Adding driver package(s)...");
    let result = pnputil::invoke(&arguments)?;
    ui::set_busy(false);

    let summary = output_tail(&result.output, 12);
    if result.success {
        let note = if result.reboot_required { "\n\nRestart the computer to finish." } else { "" };
        show_message(&format!("Done.\n\n{summary}{note}"), Icon::Information);
    } else {
        show_message(
            &format!("pnputil reported a problem (exit code {}):\n\n{}\n\nSee the log for the full output.", result.exit_code, summary),
            Icon::Warning,
        );
    }
    update_inventory()
}

// ---- Drivers > Add only newer driver packages... ---------------------------------------------------------

/// Lines of a list, at most `max`, then "... and N more".
fn limited_lines(lines: &[String], max: usize) -> String {
    let mut shown: Vec<String> = lines.iter().take(max).cloned().collect();
    if lines.len() > max {
        shown.push(format!("  ... and {} more", lines.len() - max));
    }
    shown.join("\n")
}

/// The "Add only newer packages" box of the folder dialog is ticked: reads each .inf file of the folder (DISM
/// does not need them installed) and decides with the same rules that mark old packages in the list
/// (model::decide_additions): newer ones are added, older or identical ones are skipped, and the user is asked
/// about the rest.
fn add_newer_driver_packages(install: bool, folder: PathBuf) -> Result<()> {
    let target = image();
    let folder_text = path_text(&folder);
    let infs = fsops::list_inf_files(&folder).map_err(|e| anyhow!("Could not search '{}': {}", folder_text, clean_io(&e)))?;
    if infs.is_empty() {
        applog::warn(&format!("No .inf files found in {folder_text}"));
        show_message(&format!("No .inf files were found in:\n{folder_text}"), Icon::Warning);
        return Ok(());
    }
    // The comparison needs the list as it is now.
    update_inventory()?;

    ui::set_busy(true);
    ui::set_status_text("Reading the .inf files...");
    let read = native::get_inf_info(&target, &infs);
    ui::set_busy(false);
    let read = read?;

    let shown = |path: &Path| path.strip_prefix(&folder).map(path_text).unwrap_or_else(|_| path_text(path));
    let candidates: Vec<std::result::Result<model::InfCandidate, String>> = read
        .iter()
        .map(|(path, outcome)| {
            outcome.as_ref().map_err(|e| e.clone()).map(|info| model::InfCandidate {
                path: path_text(path),
                original_inf: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                class: info.class.clone(),
                provider: info.provider.clone(),
                version: info.version,
                date: info.date,
            })
        })
        .collect();
    let decisions = model::decide_additions(&drivers_snapshot(), &candidates);

    applog::info(&format!("Read {} .inf file(s) from {folder_text} with DISM:", infs.len()));
    for (((path, _), candidate), decision) in read.iter().zip(&candidates).zip(&decisions) {
        let data = match candidate {
            Ok(c) => format!("class={} provider={} version={} date={}", c.class, c.provider, c.version, format_date(c.date)),
            Err(error) => format!("not read: {error}"),
        };
        applog::info(&format!("  {}  {}  ->  {:?}", shown(path), data, decision));
    }

    let mut to_add: Vec<usize> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut to_ask: Vec<usize> = Vec::new();
    for (index, decision) in decisions.iter().enumerate() {
        match decision {
            model::AddDecision::Add => to_add.push(index),
            model::AddDecision::Skip(reason) => skipped.push(format!("  {} ({reason})", shown(&infs[index]))),
            model::AddDecision::Ask(_) => to_ask.push(index),
        }
    }

    if to_add.is_empty() && to_ask.is_empty() {
        applog::info(&format!("Nothing to add: all {} .inf file(s) are skipped.", infs.len()));
        show_message(
            &format!(
                "Nothing to add: none of the {} .inf file(s) in\n{folder_text}\nis newer than what is installed.\n\nSkipped:\n{}",
                infs.len(),
                limited_lines(&skipped, 12)
            ),
            Icon::Information,
        );
        return Ok(());
    }

    let mut question = format!(
        "{} .inf file(s) in:\n{folder_text}\n\nTo add: {}.  Skipped (not newer): {}.  To decide: {}.\n\n",
        infs.len(),
        to_add.len(),
        skipped.len(),
        to_ask.len()
    );
    if install {
        question.push_str("The files to add go into the Driver Store AND are installed on matching devices. Windows installs a package on a device only if it is the best match for it.");
    } else {
        question.push_str("The files to add go into the Driver Store only (no device will be changed).");
    }
    if !skipped.is_empty() {
        question.push_str(&format!("\n\nSKIPPED - not newer than what is installed:\n{}", limited_lines(&skipped, 12)));
    }
    if !to_ask.is_empty() {
        let lines: Vec<String> = to_ask.iter().map(|&i| format!("  {}", shown(&infs[i]))).collect();
        question.push_str(&format!("\n\nTO DECIDE - you will be asked about each one next:\n{}", limited_lines(&lines, 12)));
    }
    if !confirm_action(&format!("{question}\n\nContinue?"), Icon::Question) {
        applog::info("Adding newer driver packages was cancelled by the user (confirmation).");
        return Ok(());
    }

    let mut chosen: Vec<usize> = to_add;
    for &index in &to_ask {
        let reason = match &decisions[index] {
            model::AddDecision::Ask(reason) => reason.as_str(),
            _ => "",
        };
        if confirm_action(
            &format!("The rules cannot decide about this file:\n\n{}\n\n{reason}.\n\nAdd it?", shown(&infs[index])),
            Icon::Question,
        ) {
            chosen.push(index);
        } else {
            applog::info(&format!("Not added (user's choice): {}", shown(&infs[index])));
            skipped.push(format!("  {} (not added, your choice)", shown(&infs[index])));
        }
    }
    chosen.sort_unstable();
    if chosen.is_empty() {
        show_message("Nothing was added.", Icon::Information);
        return Ok(());
    }

    applog::info(&format!("Adding {} of {} .inf file(s) from {folder_text} (install on devices: {}).", chosen.len(), infs.len(), ps_bool(install)));
    ui::set_busy(true);
    ui::show_progress(chosen.len());
    let (mut added, mut failed, mut reboot) = (0usize, 0usize, false);
    let mut failures: Vec<String> = Vec::new();
    if target.is_offline() {
        ui::set_status_text("Adding driver package(s) to the image...");
        let paths: Vec<PathBuf> = chosen.iter().map(|&i| infs[i].clone()).collect();
        match native::add_drivers_offline(&target, &paths) {
            Ok(results) => {
                for (path, outcome) in results {
                    match outcome {
                        Ok(()) => added += 1,
                        Err(error) => {
                            failed += 1;
                            applog::error(&format!("Could not add {}: {error}", path.display()));
                            failures.push(format!("{}: {error}", shown(&path)));
                        }
                    }
                    ui::step_progress();
                }
            }
            Err(error) => {
                ui::hide_progress();
                ui::set_busy(false);
                return Err(error);
            }
        }
    } else {
        for &index in &chosen {
            ui::set_status_text(&format!("Adding {}...", shown(&infs[index])));
            let mut arguments = vec!["/add-driver".to_string(), path_text(&infs[index])];
            if install {
                arguments.push("/install".to_string());
            }
            match pnputil::invoke(&arguments) {
                Ok(result) if result.success => {
                    added += 1;
                    reboot |= result.reboot_required;
                }
                Ok(result) => {
                    failed += 1;
                    applog::error(&format!("Could not add {} (pnputil exit code {}).", infs[index].display(), result.exit_code));
                    failures.push(format!("{} (exit code {})", shown(&infs[index]), result.exit_code));
                }
                Err(error) => {
                    failed += 1;
                    applog::error(&format!("Could not add {}: {error}", infs[index].display()));
                    failures.push(format!("{}: {error}", shown(&infs[index])));
                }
            }
            ui::step_progress();
        }
    }
    ui::hide_progress();
    ui::set_busy(false);

    let mut summary = format!("Done: {added} added, {} skipped, {failed} failed.", skipped.len());
    if failed > 0 {
        summary.push_str(&format!("\n\nFailed:\n{}\n\nThe reason for each failure is in the log.", limited_lines(&failures.iter().map(|f| format!("  {f}")).collect::<Vec<_>>(), 8)));
    }
    if reboot {
        summary.push_str("\n\nRestart the computer to finish.");
    }
    applog::info(&format!("Add newer finished: {added} added, {} skipped, {failed} failed.", skipped.len()));
    update_inventory()?;
    show_message(&summary, if failed == 0 { Icon::Information } else { Icon::Warning });
    Ok(())
}

// ---- Drivers > Export ... --------------------------------------------------------------------------------

/// Copies the given packages to a destination folder. Returns the number of exported packages.
fn export_driver_packages_to(packages: &[Driver], destination: &Path) -> Result<usize> {
    let mut exported = 0usize;
    let mut manifest: Vec<ManifestPackage> = Vec::new();
    for package in packages {
        ui::set_status_text(&format!("Exporting {} ({})...", package.published_name, package.original_inf));
        let copied = fsops::copy_driver_package(package, destination)?;
        applog::info(&format!("Exported {} to {} (every file verified by SHA-256).", package.published_name, path_text(&copied.target)));
        // The manifest makes the export restorable with Drivers > Restore backup..., and checkable.
        manifest.push(backup::manifest_package(package, destination, &copied.target, copied.files).map_err(|e| anyhow!("{e}"))?);
        backup::write_manifest(destination, &manifest)?;
        exported += 1;
        ui::step_progress();
    }
    Ok(exported)
}

fn export_driver_packages(all: bool) -> Result<()> {
    let packages: Vec<Driver> = DRIVERS.with(|d| d.borrow().iter().filter(|p| all || p.checked).cloned().collect());
    export_packages(
        packages,
        "No packages are checked. Check the packages to export first, or use \"Export all driver packages\".",
    )
}

/// Right-click > Export selected driver packages: the highlighted rows, whether checked or not.
fn export_selected_packages() -> Result<()> {
    export_packages(selected_packages(), "No rows are selected. Click a row (or several, with Ctrl or Shift) first.")
}

/// Asks for a folder and copies the given packages into a new, time-stamped folder inside it.
fn export_packages(packages: Vec<Driver>, nothing_message: &str) -> Result<()> {
    if packages.is_empty() {
        show_message(nothing_message, Icon::Information);
        return Ok(());
    }

    let Some(selected) = win::browse_for_folder(
        ui::form(),
        &format!("Select the folder where the {} package(s) will be exported (one subfolder per package).", packages.len()),
    ) else {
        applog::info("Export of driver packages was cancelled by the user.");
        return Ok(());
    };

    let bytes: u64 = packages.iter().map(|p| p.size_bytes).sum();
    if !confirm_action(
        &format!(
            "Export {} package(s) ({}) into a new folder named {APP_FILE_NAME}_export_<time> inside:\n{}\n\nContinue?",
            packages.len(),
            format_size(bytes as f64),
            path_text(&selected)
        ),
        Icon::Question,
    ) {
        applog::info("Export of driver packages was cancelled by the user.");
        return Ok(());
    }

    // The folder is named after the moment the export starts, so repeated exports never mix.
    let destination = selected.join(format!("{APP_FILE_NAME}_export_{}", applog::file_stamp()));
    applog::info(&format!("Exporting {} package(s) to {}.", packages.len(), path_text(&destination)));
    ui::set_busy(true);
    ui::show_progress(packages.len());
    let exported = export_driver_packages_to(&packages, &destination)?;
    ui::hide_progress();
    ui::set_busy(false);
    update_summary();

    applog::info(&format!("Export finished: {exported} package(s)."));
    show_message(
        &format!(
            "Exported {exported} package(s) (every file verified by SHA-256) to:\n{}\n\nTo restore them, use Drivers > Restore backup... and pick this folder. To add just one, use Drivers > Add driver package...",
            path_text(&destination)
        ),
        Icon::Information,
    );
    Ok(())
}

// ---- Drivers > Restore backup... -------------------------------------------------------------------------

/// Asks for a backup or export folder and reads its manifest. None = cancelled, or a message already said why.
fn pick_backup(action: &str) -> Option<(PathBuf, Vec<ManifestPackage>)> {
    let Some(folder) = win::browse_for_folder(
        ui::form(),
        "Select a backup folder made by this program: a folder with a manifest.txt inside, one per removal in the backup folder, or an export folder.",
    ) else {
        applog::info(&format!("{action} was cancelled by the user (folder dialog)."));
        return None;
    };
    let folder_text = path_text(&folder);

    if !folder.join(backup::MANIFEST_NAME).exists() {
        show_message(
            &format!("There is no {} in:\n{folder_text}\n\nOnly backups and exports made by this version have one. To add packages from any other folder, use Drivers > Add driver package...", backup::MANIFEST_NAME),
            Icon::Information,
        );
        return None;
    }
    match backup::read_manifest(&folder) {
        Ok(packages) => Some((folder, packages)),
        Err(error) => {
            applog::error(&format!("The manifest in {folder_text} is not valid: {error}"));
            show_message(&format!("The manifest in:\n{folder_text}\nis not valid:\n{error}\n\nNothing was changed."), Icon::Warning);
            None
        }
    }
}

/// Checks every file of each package against the SHA-256 recorded in the manifest. Returns the packages that
/// are intact and a "name: problem" line for each one that is not. The caller shows the progress bar.
fn verify_packages<'a>(folder: &Path, packages: &'a [ManifestPackage]) -> (Vec<&'a ManifestPackage>, Vec<String>) {
    let mut intact = Vec::new();
    let mut damaged = Vec::new();
    for package in packages {
        ui::set_status_text(&format!("Checking {} ({})...", package.published_name, package.inf));
        match backup::verify_package(folder, package) {
            Ok(()) => {
                applog::info(&format!("Backup of {} is intact (every file matches its SHA-256).", package.published_name));
                intact.push(package);
            }
            Err(problem) => {
                applog::error(&format!("Backup of {} is NOT intact: {problem}", package.published_name));
                damaged.push(format!("{}: {problem}", package.published_name));
            }
        }
        ui::step_progress();
    }
    (intact, damaged)
}

/// Drivers > Verify backup...: the check that Restore does first, without restoring anything.
fn verify_backup() -> Result<()> {
    let Some((folder, packages)) = pick_backup("Verify backup") else { return Ok(()) };
    applog::info(&format!("Verifying {} package(s) of {}.", packages.len(), path_text(&folder)));
    ui::set_busy(true);
    ui::show_progress(packages.len());
    let (intact, damaged) = verify_packages(&folder, &packages);
    ui::hide_progress();
    ui::set_busy(false);

    let mut summary = format!("{} of {} package(s) are intact (every file matches its SHA-256).", intact.len(), packages.len());
    if !damaged.is_empty() {
        summary.push_str(&format!("\n\nNot intact:\n{}", damaged.join("\n")));
    }
    applog::info(&format!("Verify finished: {} intact, {} not intact.", intact.len(), damaged.len()));
    show_message(&summary, if damaged.is_empty() { Icon::Information } else { Icon::Warning });
    Ok(())
}

/// Restores the packages of a backup (or export) folder that has a manifest: every file is checked against
/// the SHA-256 recorded when the backup was made, and only packages that are intact are added back to the
/// Driver Store and installed on the devices they match.
fn restore_backup() -> Result<()> {
    let Some((folder, packages)) = pick_backup("Restore backup") else { return Ok(()) };
    let folder_text = path_text(&folder);

    let list: Vec<(String, String)> =
        packages.iter().map(|p| (p.published_name.clone(), format!("{} v{}, {} device(s) used it", p.inf, p.version, p.devices.len()))).collect();
    if !confirm_action(
        &format!(
            "Restore {} package(s) from:\n{folder_text}\n\n{}\n\nEvery file is checked against the SHA-256 recorded in the backup first; a package that is not intact is skipped. {}\n\nContinue?",
            packages.len(),
            format_package_list(&list),
            if image().is_offline() {
                format!("The others are added to {} (no device is touched).", image().describe())
            } else {
                "The others are added to the Driver Store AND installed on matching devices (Windows installs a package on a device only if it is the best match for it).".to_string()
            }
        ),
        Icon::Question,
    ) {
        applog::info("Restore backup was cancelled by the user (confirmation).");
        return Ok(());
    }

    applog::info(&format!("Restoring {} package(s) from {folder_text}.", packages.len()));
    ui::set_busy(true);
    ui::show_progress(packages.len() * 2);
    let (intact, damaged) = verify_packages(&folder, &packages);

    let (mut added, mut failed, mut reboot) = (0usize, 0usize, false);
    let target = image();
    if target.is_offline() {
        // An offline image is changed through the DISM API, all the intact packages in one session.
        let infs: Vec<PathBuf> =
            intact.iter().map(|p| backup::join_relative(&folder, &p.folder).join(&p.inf)).collect();
        ui::set_status_text("Restoring the packages into the image...");
        let results = match native::add_drivers_offline(&target, &infs) {
            Ok(results) => results,
            Err(error) => {
                ui::hide_progress();
                ui::set_busy(false);
                return Err(error);
            }
        };
        for (inf, outcome) in results {
            match outcome {
                Ok(()) => added += 1,
                Err(error) => {
                    failed += 1;
                    applog::error(&format!("Could not restore {}: {error}", inf.display()));
                }
            }
            ui::step_progress();
        }
    } else {
        for package in intact {
            ui::set_status_text(&format!("Restoring {} ({})...", package.published_name, package.inf));
            let inf = backup::join_relative(&folder, &package.folder).join(&package.inf);
            let result = pnputil::invoke(&["/add-driver".to_string(), path_text(&inf), "/install".to_string()])?;
            if result.success {
                added += 1;
                reboot |= result.reboot_required;
            } else {
                failed += 1;
                applog::error(&format!("Could not restore {} (see the pnputil output above).", package.published_name));
            }
            ui::step_progress();
        }
    }
    ui::hide_progress();
    ui::set_busy(false);

    let mut summary = format!("Done: {added} restored, {failed} failed, {} skipped because the backup is not intact.", damaged.len());
    if !damaged.is_empty() {
        summary.push_str(&format!("\n\nNot intact:\n{}", damaged.join("\n")));
    }
    if failed > 0 {
        summary.push_str("\n\nThe reason for each failure is in the log.");
    }
    if reboot {
        summary.push_str("\n\nRestart the computer to finish.");
    }
    applog::info(&format!("Restore finished: {added} restored, {failed} failed, {} skipped.", damaged.len()));
    update_inventory()?;
    show_message(&summary, Icon::Information);
    Ok(())
}

// ---- Drivers > Remove checked driver packages... ---------------------------------------------------------

struct RemoveResult {
    removed: i64,
    failed: i64,
    reboot_required: bool,
    /// Packages for which pnputil returned exit code 0. After a restart-required code (3010) the package
    /// may legitimately still be listed, so those are not re-checked.
    cleanly_removed: Vec<Driver>,
}

/// `^oem\d+\.inf$` without regex (case-insensitive; like .NET, `$` also matches before one final \n).
fn is_published_name(name: &str) -> bool {
    let name = name.strip_suffix('\n').unwrap_or(name).to_lowercase();
    match name.strip_prefix("oem").and_then(|rest| rest.strip_suffix(".inf")) {
        Some(digits) => !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Removes the given packages one by one. A failure on one package is logged and counted; the remaining
/// packages are still processed. If `backup_directory` is given, each package is backed up first and is
/// NOT removed when its backup fails. A package that is still in use is removed too: `/uninstall` first
/// uninstalls the driver from the devices that use it (pnputil ignores /force together with /uninstall).
fn remove_driver_packages(target: &ImageTarget, packages: &[Driver], backup_directory: Option<&Path>) -> Result<RemoveResult> {
    // Only real published names (oemNN.inf) may reach pnputil. Checked for ALL packages before the first removal.
    for package in packages {
        if !is_published_name(&package.published_name) {
            bail!(
                "Refusing to remove '{}': it is not a published driver package name (oemNN.inf).",
                package.published_name
            );
        }
    }

    let mut result = RemoveResult { removed: 0, failed: 0, reboot_required: false, cleanly_removed: Vec::new() };
    let mut manifest: Vec<ManifestPackage> = Vec::new();

    for package in packages {
        let name = &package.published_name;
        ui::set_status_text(&format!("Removing {name} ({})...", package.original_inf));

        if let Some(directory) = backup_directory {
            let backed_up = fsops::copy_driver_package(package, directory).and_then(|copied| {
                // The manifest records the devices and the SHA-256 of every file, for Drivers > Restore backup...
                let entry = backup::manifest_package(package, directory, &copied.target, copied.files).map_err(|e| anyhow!("{e}"))?;
                manifest.push(entry);
                backup::write_manifest(directory, &manifest)?;
                Ok(copied.target)
            });
            match backed_up {
                Ok(target) => applog::info(&format!("Backed up {name} to {} (every file verified by SHA-256).", path_text(&target))),
                Err(error) => {
                    applog::error(&format!("{name} was NOT removed because its backup failed: {error}"));
                    result.failed += 1;
                    ui::step_progress();
                    continue;
                }
            }
        }

        if target.is_offline() {
            // An offline image is changed through the DISM API: pnputil only works on the running Windows.
            applog::info(&format!("Removing {name} ({} v{}) from {}.", package.original_inf, package.version, target.describe()));
            match native::remove_driver_offline(target, name) {
                Ok(()) => {
                    result.removed += 1;
                    result.cleanly_removed.push(package.clone());
                    applog::info(&format!("DISM reports that {name} was removed."));
                }
                Err(error) => {
                    result.failed += 1;
                    applog::error(&format!("Could not remove {name}: {error}"));
                }
            }
            ui::step_progress();
            continue;
        }
        applog::info(&format!(
            "Removing {name} ({} v{}). This uninstalls the driver from the devices that use it, then deletes the package.",
            package.original_inf, package.version
        ));
        let arguments = vec!["/delete-driver".to_string(), name.clone(), "/uninstall".to_string()];
        let command = pnputil::invoke(&arguments)?;
        if command.success {
            result.removed += 1;
            if command.reboot_required {
                result.reboot_required = true;
            } else {
                result.cleanly_removed.push(package.clone());
            }
            applog::info(&format!("pnputil reports that {name} was removed."));
        } else {
            result.failed += 1;
            applog::error(&format!("Could not remove {name} (see the pnputil output above)."));
        }
        ui::step_progress();
    }
    Ok(result)
}

/// Driver Store numbers (oemNN.inf) are reused by Windows, so the list on screen can go stale (for example
/// when another program installs a driver). Returns the names of the given packages whose current Driver
/// Store entry is missing or is not the same package (INF name, version, date) as the one on screen.
fn get_changed_packages(target: &ImageTarget, packages: &[Driver]) -> Result<Vec<String>> {
    let mut current: HashMap<String, String> = HashMap::new();
    for entry in drvstore::get_windows_drivers(target)? {
        current.insert(entry.driver.to_lowercase(), format!("{}|{}|{}", entry.original_inf, entry.version, format_date(entry.date)));
    }
    let mut changed = Vec::new();
    for package in packages {
        let expected = format!("{}|{}|{}", package.original_inf, package.version, format_date(package.date));
        let matches = current
            .get(&package.published_name.to_lowercase())
            .map(|found| found.to_lowercase() == expected.to_lowercase())
            .unwrap_or(false);
        if !matches {
            changed.push(package.published_name.clone());
        }
    }
    Ok(changed)
}

fn remove_checked_packages() -> Result<()> {
    let selected: Vec<Driver> = DRIVERS.with(|d| d.borrow().iter().filter(|p| p.checked).cloned().collect());
    remove_packages(
        selected,
        "No packages are checked. Use the Select menu or tick the boxes in the list first.",
    )
}

/// Right-click > Remove selected driver packages: the highlighted rows, whether checked or not.
fn remove_selected_packages() -> Result<()> {
    remove_packages(selected_packages(), "No rows are selected. Click a row (or several, with Ctrl or Shift) first.")
}

/// Asks for confirmation (with the relevant warnings), checks that the Driver Store still matches the
/// list, then removes.
/// Right-click > Protect / Unprotect: if every selected package is protected they are all unprotected,
/// otherwise they are all protected. A protected package is not checked by the automatic rules and is
/// skipped by every removal.
fn toggle_protection() -> Result<()> {
    let selected = selected_packages();
    if selected.is_empty() {
        show_message("No rows are selected. Click a row (or several, with Ctrl or Shift) first.", Icon::Information);
        return Ok(());
    }
    let protect = selected.iter().any(|p| !p.protected);
    let keys: Vec<String> = selected.iter().map(model::protection_key).collect();
    PROTECTED.with(|p| {
        let mut list = p.borrow_mut();
        list.retain(|k| !keys.contains(k));
        if protect {
            list.extend(keys.iter().cloned());
        }
    });
    DRIVERS.with(|d| {
        for package in d.borrow_mut().iter_mut() {
            if keys.contains(&model::protection_key(package)) {
                package.protected = protect;
                if protect {
                    package.checked = false;
                }
            }
        }
    });
    applog::info(&format!(
        "{} {} package(s): {}",
        if protect { "Protected" } else { "Unprotected" },
        keys.len(),
        selected.iter().map(|p| format!("{} ({})", p.published_name, p.original_inf)).collect::<Vec<_>>().join(", ")
    ));
    update_view();
    Ok(())
}

/// Right-click > Remove disconnected devices of selected packages...: removes, with `pnputil /remove-device`,
/// the devices of the selected packages that are not plugged in. A disconnected device keeps its package "in
/// use"; once it is removed the package can be cleaned up. The packages themselves are not removed. Just before
/// the removal the devices are read again: one that is plugged in by now, or is not found any more, is skipped.
fn remove_disconnected_devices() -> Result<()> {
    if image().is_offline() {
        show_message("Not available for an offline image: it has no devices.", Icon::Information);
        return Ok(());
    }
    let selected = selected_packages();
    if selected.is_empty() {
        show_message("No rows are selected. Click a row (or several, with Ctrl or Shift) first.", Icon::Information);
        return Ok(());
    }
    let mut devices: Vec<model::DeviceRef> = Vec::new();
    for package in &selected {
        for device in &package.absent_devices {
            if !devices.iter().any(|d| d.instance_id.eq_ignore_ascii_case(&device.instance_id)) {
                devices.push(device.clone());
            }
        }
    }
    if devices.is_empty() {
        show_message("None of the selected packages is used by a disconnected device.", Icon::Information);
        return Ok(());
    }

    let lines: Vec<String> = devices.iter().map(|d| format!("  {} ({})", d.name, d.instance_id)).collect();
    if !confirm_action(
        &format!(
            "Remove {} disconnected device(s) from Windows?\n\nEach one is removed with pnputil /remove-device. Windows creates the device again, and installs a driver for it, the next time it is plugged in. The driver packages are not removed.\n\n{}\n\nContinue?",
            devices.len(),
            limited_lines(&lines, 12)
        ),
        Icon::Question,
    ) {
        applog::info("Removing disconnected devices was cancelled by the user.");
        return Ok(());
    }

    ui::set_busy(true);
    ui::set_status_text("Checking that the devices are still disconnected...");
    let current = native::get_devices();
    let current = match current {
        Ok(list) => list,
        Err(error) => {
            ui::set_busy(false);
            return Err(error);
        }
    };
    let presence: HashMap<String, bool> = current.iter().map(|d| (d.instance_id.to_lowercase(), d.present)).collect();

    let (mut removed, mut failed, mut reboot) = (0usize, 0usize, false);
    let mut skipped: Vec<String> = Vec::new();
    for device in &devices {
        match presence.get(&device.instance_id.to_lowercase()) {
            Some(false) => {}
            Some(true) => {
                applog::warn(&format!("Not removed, it is plugged in now: {}", device.instance_id));
                skipped.push(format!("  {} (plugged in now)", device.name));
                continue;
            }
            None => {
                applog::warn(&format!("Not removed, it was not found any more: {}", device.instance_id));
                skipped.push(format!("  {} (not found any more)", device.name));
                continue;
            }
        }
        ui::set_status_text(&format!("Removing {}...", device.name));
        match pnputil::invoke(&["/remove-device".to_string(), device.instance_id.clone()]) {
            Ok(result) if result.success => {
                removed += 1;
                reboot |= result.reboot_required;
            }
            Ok(result) => {
                failed += 1;
                applog::error(&format!("Could not remove {} (pnputil exit code {}).", device.instance_id, result.exit_code));
            }
            Err(error) => {
                failed += 1;
                applog::error(&format!("Could not remove {}: {error}", device.instance_id));
            }
        }
    }
    ui::set_busy(false);

    applog::info(&format!("Disconnected devices: {removed} removed, {} skipped, {failed} failed.", skipped.len()));
    let mut summary = format!("Done: {removed} device(s) removed, {} skipped, {failed} failed.", skipped.len());
    if !skipped.is_empty() {
        summary.push_str(&format!("\n\nSkipped:\n{}", limited_lines(&skipped, 8)));
    }
    if failed > 0 {
        summary.push_str("\n\nThe reason for each failure is in the log.");
    }
    if reboot {
        summary.push_str("\n\nRestart the computer to finish.");
    }
    update_inventory()?;
    show_message(&summary, if failed == 0 { Icon::Information } else { Icon::Warning });
    Ok(())
}

fn remove_packages(selected: Vec<Driver>, nothing_message: &str) -> Result<()> {
    if selected.is_empty() {
        show_message(nothing_message, Icon::Information);
        return Ok(());
    }
    // Protected packages are never removed, however they were selected.
    let (protected, selected): (Vec<Driver>, Vec<Driver>) = selected.into_iter().partition(|p| p.protected);
    if selected.is_empty() {
        show_message(
            "Nothing was removed: every selected package is protected. Unprotect it first (right-click > Protect / Unprotect selected packages).",
            Icon::Information,
        );
        return Ok(());
    }

    let target = image();
    let offline = target.is_offline();
    let back_up = ui::menu_checked(ID_OPT_BACKUP);
    // Where the backups go: next to the program for the running Windows; a folder you choose for an image.
    let backup_root: Option<PathBuf> = if !back_up {
        None
    } else if offline {
        match offline_backup_root()? {
            Some(path) => Some(path),
            None => {
                applog::info("Removal cancelled by the user (no backup folder chosen).");
                return Ok(());
            }
        }
    } else {
        Some(applog::backup_root().to_path_buf())
    };
    let in_use: Vec<&Driver> = selected.iter().filter(|p| p.in_use).collect();
    // Not superseded and not a duplicate: nothing else would take over from this package.
    let not_old_in_use: Vec<&Driver> = selected.iter().filter(|p| !p.is_old && p.in_use).collect();
    let not_old_unused: Vec<&Driver> = selected.iter().filter(|p| !p.is_old && !p.in_use).collect();
    let boot: Vec<&Driver> = selected.iter().filter(|p| p.boot_critical).collect();

    let mut message = if offline {
        format!(
            "Remove {} driver package(s) from the OFFLINE image {}?\n\nThe packages are removed from the image only. An offline image has no devices, so it is not known which devices need them: check that nothing in that Windows depends on them.\n\n",
            selected.len(),
            target.describe().trim_start_matches("the offline image ")
        )
    } else {
        format!(
            "Remove {} driver package(s)?\n\nFor each package, Windows first uninstalls the driver from the devices that use it (they switch to the best remaining driver), then deletes the package from the Driver Store.\n\n",
            selected.len()
        )
    };
    if let Some(root) = &backup_root {
        message.push_str(&format!(
            "A backup of each package is saved first, in a new folder named after the time the removal starts, inside:\n{}",
            path_text(root)
        ));
    } else {
        message.push_str("No backup will be made (Options > Back up packages before removing is off).");
    }

    if !in_use.is_empty() {
        message.push_str(&format!(
            "\n\nIN USE: {} package(s) are still bound to a device. That device will switch to the best remaining driver:\n{}",
            in_use.len(),
            format_package_list(&pairs(&in_use))
        ));
    }
    if !not_old_in_use.is_empty() {
        message.push_str(&format!(
            "\n\nNOT OLD AND IN USE: {} package(s) are not clearly superseded by a newer package (they are the latest, or their version and date disagree), so their devices may be left WITHOUT A DRIVER:\n{}",
            not_old_in_use.len(),
            format_package_list(&pairs(&not_old_in_use))
        ));
    }
    if offline && selected.iter().any(|p| !p.is_old) {
        let not_old: Vec<&Driver> = selected.iter().filter(|p| !p.is_old).collect();
        message.push_str(&format!(
            "\n\nNOT OLD: {} package(s) are not superseded by a newer package in this image (they are the latest, or need review):\n{}",
            not_old.len(),
            format_package_list(&pairs(&not_old))
        ));
    }
    if !offline && !not_old_unused.is_empty() {
        message.push_str(&format!(
            "\n\nNOT OLD, BUT UNUSED: {} package(s) are not superseded by a newer package. No device uses them now, but a device plugged in later will need its driver installed again:\n{}",
            not_old_unused.len(),
            format_package_list(&pairs(&not_old_unused))
        ));
    }
    if !boot.is_empty() {
        message.push_str(&format!(
            "\n\nBOOT-CRITICAL: {} package(s) are needed to start Windows:\n{}",
            boot.len(),
            format_package_list(&pairs(&boot))
        ));
    }

    if !protected.is_empty() {
        let list: Vec<&Driver> = protected.iter().collect();
        message.push_str(&format!(
            "\n\nPROTECTED: {} selected package(s) are protected and will NOT be removed:\n{}",
            list.len(),
            format_package_list(&pairs(&list))
        ));
    }

    let icon = if !not_old_in_use.is_empty() || !boot.is_empty() { Icon::Warning } else { Icon::Question };
    if !confirm_action(&message, icon) {
        applog::info("Removal cancelled by the user.");
        return Ok(());
    }

    applog::info(&format!(
        "Removal confirmed for {} package(s) (backup: {}):",
        selected.len(),
        ps_bool(back_up)
    ));
    for package in &selected {
        applog::info(&format!(
            "  {}  {} v{}  usage={}  {}",
            package.published_name, package.original_inf, package.version, package.usage_text, package.status_text
        ));
    }

    // Safety check: the list may be older than the Driver Store (Windows reuses the oemNN numbers).
    ui::set_busy(true);
    ui::set_status_text("Checking that the selected packages have not changed...");
    applog::info("Verifying that the selected packages still match the Driver Store.");
    let changed = get_changed_packages(&target, &selected)?;
    if !changed.is_empty() {
        applog::warn(&format!(
            "Nothing was removed: these entries changed after the list was loaded: {}",
            changed.join(", ")
        ));
        ui::set_busy(false);
        show_message(
            &format!(
                "Nothing was removed.\n\nThe Driver Store changed after the list was loaded (Windows reuses oemNN numbers), so these entries no longer match what you selected:\n{}\n\nThe list will now be refreshed. Select the packages again.",
                changed.join(", ")
            ),
            Icon::Warning,
        );
        return update_inventory();
    }

    // The backup folder is named after the moment the removal starts. The backup root must be a real folder.
    let backup_directory: Option<PathBuf> = if let Some(root) = &backup_root {
        if let Err(error) = fsops::ensure_plain_dir(root) {
            ui::set_busy(false);
            return Err(error);
        }
        Some(root.join(applog::file_stamp()))
    } else {
        None
    };

    ui::show_progress(selected.len());
    let mut result = remove_driver_packages(&target, &selected, backup_directory.as_deref())?;

    if !offline {
        applog::info("Rescanning devices (pnputil /scan-devices) so that devices pick up the remaining driver packages.");
        ui::set_status_text("Rescanning devices...");
        let _ = pnputil::invoke(&["/scan-devices".to_string()])?;
    }
    ui::hide_progress();
    ui::set_busy(false);

    // Verify the outcome instead of trusting pnputil alone: reload the Driver Store and make sure that
    // every package reported as removed is really gone.
    update_inventory()?;
    let still_listed: Vec<Driver> = DRIVERS.with(|d| {
        let current = d.borrow();
        result.cleanly_removed.iter().filter(|p| is_package_listed(p, &current)).cloned().collect()
    });
    for package in &still_listed {
        result.removed -= 1;
        result.failed += 1;
        applog::error(&format!(
            "{} reported success for {}, but the package is still in the Driver Store.",
            if offline { "DISM" } else { "pnputil" },
            package.published_name
        ));
    }

    applog::info(&format!("Removal finished: {} removed, {} failed.", result.removed, result.failed));
    let mut summary = format!("Done: {} removed, {} failed.", result.removed, result.failed);
    if !still_listed.is_empty() {
        let refs: Vec<&Driver> = still_listed.iter().collect();
        summary.push_str(&format!(
            "\n\n{} reported success, but these packages are still in the Driver Store:\n{}",
            if offline { "DISM" } else { "pnputil" },
            format_package_list(&pairs(&refs))
        ));
    }
    if result.failed > 0 {
        summary.push_str("\n\nFailed packages stay in the list. The reason is in the log.");
    }
    if let Some(directory) = &backup_directory {
        summary.push_str(&format!("\n\nBackups: {}", path_text(directory)));
    }
    if result.reboot_required {
        summary.push_str("\n\nRestart the computer to finish.");
    }
    if applog::is_broken() {
        summary.push_str("\n\nWARNING: the log file could not be written, so details of this run are not saved.");
    } else {
        summary.push_str(&format!("\n\nLog: {}", path_text(applog::log_file())));
    }
    show_message(&summary, Icon::Information);
    Ok(())
}

// ---- Help ------------------------------------------------------------------------------------------------

fn open_folder(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| anyhow!("Could not create '{}': {}", path_text(path), clean_io(&e)))?;
    win::start_explorer(&format!("\"{}\"", path_text(path)))
}

fn show_how_it_works() {
    let lines: Vec<String> = vec![
        "WHAT YOU SEE".into(),
        "Every third-party driver package (oemNN.inf) stored in the Windows Driver Store. \"Published name\" is the name Windows gave the package; \"Original INF\" is the vendor file name. A size ending in + means part of the package could not be read, so the real size is larger. \"Signature\" is the class Windows gives the signature (Logo Premium, Logo Standard, WHQL, Inbox, Unclassified, Authenticode, Unsigned...) and \"Signer\" who signed it; \"Install date (UTC)\" is when the package was added to the Driver Store, in UTC so it reads the same online and offline; \"Extension ID\" is set only for extension INFs; \"Driver files\" counts the files in the package folder and names the first five (the full list is the folder: right-click > Open package folder); \"Driver path\" is the package folder in the Driver Store.".into(),
        "".into(),
        "OLD".into(),
        "Either another package of the same driver (same class, provider, INF name and extension ID) has a newer date and a version that is not lower, or an identical package (same version and date) is kept instead: of identical packages one stays (the one a device uses, otherwise the lowest oemNN) and the others are duplicates.".into(),
        "".into(),
        "REVIEW".into(),
        "Date and version disagree about which package of the same driver is newer (for example a higher version with an older date). The program cannot tell which one Windows prefers, so it never selects these automatically; you decide.".into(),
        "".into(),
        "IN USE".into(),
        "At least one device is bound to the package, directly or through an extension INF. The Devices column lists them; \"not connected\" means the device is not plugged in right now, and \"problem code NN\" is the code Device Manager shows for a plugged-in device that has a problem (View > Show only packages used by devices with a problem lists those packages).".into(),
        "".into(),
        "COLORS (never the only signal)".into(),
        "Light blue = old and unused. Light yellow = old but in use. Light gray = review. No color = latest. The \"In use\" and \"Status\" columns say the same in text.".into(),
        "".into(),
        "SELECTING".into(),
        "Check old packages (unused only - safe) is the safe choice; including in use also switches devices to a newer package. Check unused packages checks every package no device uses, whatever its age. Packages of ntprint.inf (the print spooler) are never checked automatically, and boot-critical packages only when Options allows it.".into(),
        "".into(),
        "REMOVING".into(),
        "For each package: pnputil /delete-driver oemNN.inf /uninstall. The driver is first uninstalled from the devices that use it (they switch to the best remaining driver), then the package is deleted. This also works for a package that is still in use; there is no force option because pnputil ignores /force together with /uninstall.".into(),
        "".into(),
        "OFFLINE IMAGES".into(),
        "File > Open offline Windows image... manages the third-party drivers of a Windows on another disk (for example in Windows PE): the list comes from drvstore.dll; add, remove and restore go through the DISM API. Pick the root of the image, the drive or folder that contains the Windows folder. An offline image has no devices, so \"In use\" is Unknown, \"Check unused packages\" and \"Add and install\" are not available, and the backups go to a folder you choose (not the RAM disk X: of Windows PE). File > Return to the running Windows goes back.".into(),
        "".into(),
        "SAFETY".into(),
        "Every removal asks for confirmation (the default answer is No). Just before removing, the packages are compared with the Driver Store again; if anything changed, nothing is removed. Backups are verified, and a package whose backup failed is not removed. After removing, the Driver Store is read again to confirm the packages are really gone. The window cannot be closed while an operation runs. The log and backup folders are refused if they are links or junctions.".into(),
        "".into(),
        "HOW IT READS".into(),
        "Packages come from the Windows Driver Store library (drvstore.dll) and devices from the Windows Configuration Manager; no PowerShell is started. Every change is made by pnputil.exe, and success is judged by its exit code, never by its text.".into(),
        "".into(),
        "BACKUPS, EXPORTS, LOGS AND SETTINGS".into(),
        format!("Backups: {}\\<time of removal>", path_text(applog::backup_root())),
        "Every backup and export folder has a manifest.txt (the devices that used each package and the SHA-256 of every file). Drivers > Restore backup... checks each file against it and installs only the packages that are intact; Drivers > Verify backup... does the same check without restoring anything.".into(),
        format!("Exports: <folder you choose>\\{APP_FILE_NAME}_export_<time of export>"),
        format!("Log of this run: {}", path_text(applog::log_file())),
        format!("Settings: {}", path_text(&settings_file())),
        "A package copied by a backup or by Export can be restored with Drivers > Add driver package...".into(),
    ];
    ui::show_text_window(&format!("{APP_NAME} - How it works"), &lines.join("\n"));
}

fn show_about() {
    ui::show_text_window(
        APP_NAME,
        &format!(
            "{APP_NAME} {APP_VERSION}\n\nReviews, cleans up, backs up and installs driver packages in the Windows Driver Store.\nIt reads packages with the Driver Store library (drvstore.dll) and devices with the Configuration Manager; every change is made by pnputil.exe (DISM for an offline image).\n\nRequires Windows 10 version 1607 or later (64-bit) and administrator rights.\n\nINSPIRED BY\nDriver Store Explorer (RAPR) by lostindark and contributors (GPL-2.0):\nhttps://github.com/lostindark/DriverStoreExplorer\n\nA NOTE ON AI\n{}",
            model::AI_NOTICE
        ),
    );
}
