//! The package records, how they are classified (Old / Review / Latest, in use or not) and how the list is
//! filtered, sorted and grouped. No UI and no Windows here.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::date::{Date, DateTime};

use crate::culture::compare_ignore_case;
use crate::format::{contains_ordinal_ignore_case, format_date, format_date_time, format_size};
use crate::netversion::NetVersion;

// ---- Constants (section 1) -------------------------------------------------------------------------

/// Name shown to the user.
pub const APP_NAME: &str = "DriverStore Manager";
/// Name used in file and folder names (no space): DriverStoreManager_<time>.log, DriverStoreManager.ini ...
pub const APP_FILE_NAME: &str = "DriverStoreManager";
pub const APP_VERSION: &str = "1.3.0.0";

/// The note about AI shown in Help > About (the README text, except "under MIT license").
pub const AI_NOTICE: &str = "I built this project alone, as a personal project, with substantial help from AI tools — mainly Claude, and also DeepSeek and Qwen. I believe knowledge only survives past us if it's shared, and that's the spirit behind releasing this for free.\n\nI did this on my own time and dime, covering all costs myself, without asking anyone for donations.\n\nJust as I respect opinions against the use of AI, I expect the use of AI here — as a tool that helped me build this project — to be respected in return. Disrespect toward this work, toward me, or toward anyone else involved will not be tolerated.\n\nAll AI-generated content is reviewed and validated by me before being committed — I stand behind every decision to include code in this repository, regardless of how it was originally written. This project is still provided as-is, under MIT license, with no warranty of any kind.\n\nIf you're uncomfortable with AI-assisted code for any reason, you are under no obligation to use, contribute to, or engage with this project. No hard feelings — just move on.\n\nFor everyone else: bug reports and PRs are evaluated on their merits (does it work, is it correct), not on how the code was produced.";

/// Packages whose INF name is listed here are never checked by the automatic rules ("Check old packages",
/// "Check unused packages"): the print spooler uses them in ways the device list does not show, and removing
/// an older copy can break printing. (Driver Store Explorer makes the same exception for ntprint.inf.)
pub const NEVER_AUTO_SELECT_INFS: [&str; 1] = ["ntprint.inf"];

/// Windows 10 version 1607: first build that has "pnputil /add-driver", "/delete-driver" and "/export-driver".
pub const MINIMUM_WINDOWS_BUILD: i64 = 14393;

/// ERROR_SUCCESS_REBOOT_REQUIRED: the pnputil command worked, but a restart is needed to finish it.
pub const EXIT_CODE_REBOOT_REQUIRED: i32 = 3010;

/// Row colors as (R, G, B).
pub const COLOR_OLD_UNUSED: (u8, u8, u8) = (207, 226, 243); // light blue   = old, no device uses it
pub const COLOR_OLD_IN_USE: (u8, u8, u8) = (255, 242, 170); // light yellow = old, a device still uses it
pub const COLOR_REVIEW: (u8, u8, u8) = (224, 224, 224); //     light gray   = version and date disagree

/// Which package property a column sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Number,
    OriginalInf,
    Provider,
    Class,
    Version,
    Date,
    InstallDate,
    Signature,
    Signer,
    ExtensionId,
    BootCritical,
    InUse,
    Status,
    DeviceText,
    DeviceId,
    SizeBytes,
    FileCount,
    Folder,
}

pub struct ColumnDefinition {
    pub title: &'static str,
    pub width: i32,
    pub sort_by: SortBy,
}

/// The columns, in the order they are shown: what the package is (name, INF, provider, class), which version
/// and when, who signed it, whether it matters (boot, use, status), the devices, then what is on disk.
pub const COLUMN_DEFINITIONS: [ColumnDefinition; 18] = [
    ColumnDefinition { title: "Published name", width: 110, sort_by: SortBy::Number },
    ColumnDefinition { title: "Original INF", width: 220, sort_by: SortBy::OriginalInf },
    ColumnDefinition { title: "Provider", width: 160, sort_by: SortBy::Provider },
    ColumnDefinition { title: "Class", width: 140, sort_by: SortBy::Class },
    ColumnDefinition { title: "Version", width: 110, sort_by: SortBy::Version },
    ColumnDefinition { title: "Date", width: 85, sort_by: SortBy::Date },
    ColumnDefinition { title: "Install date (UTC)", width: 125, sort_by: SortBy::InstallDate },
    ColumnDefinition { title: "Signature", width: 100, sort_by: SortBy::Signature },
    ColumnDefinition { title: "Signer", width: 200, sort_by: SortBy::Signer },
    ColumnDefinition { title: "Extension ID", width: 280, sort_by: SortBy::ExtensionId },
    ColumnDefinition { title: "Boot-critical", width: 90, sort_by: SortBy::BootCritical },
    ColumnDefinition { title: "In use", width: 70, sort_by: SortBy::InUse },
    ColumnDefinition { title: "Status", width: 240, sort_by: SortBy::Status },
    ColumnDefinition { title: "Devices", width: 260, sort_by: SortBy::DeviceText },
    ColumnDefinition { title: "Device ID", width: 300, sort_by: SortBy::DeviceId },
    ColumnDefinition { title: "Size", width: 75, sort_by: SortBy::SizeBytes },
    ColumnDefinition { title: "Driver files", width: 420, sort_by: SortBy::FileCount },
    ColumnDefinition { title: "Driver path", width: 430, sort_by: SortBy::Folder },
];

/// The package property used to group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBy {
    Class,
    Provider,
    OriginalInf,
    UsageText,
    Status,
}

/// View > Group by. The value is the property used to group (None = no groups). Same order as the script.
pub const GROUP_MODES: [(&str, Option<GroupBy>); 6] = [
    ("None", None),
    ("Class (device type)", Some(GroupBy::Class)),
    ("Provider", Some(GroupBy::Provider)),
    ("Original INF", Some(GroupBy::OriginalInf)),
    ("Usage (in use / unused)", Some(GroupBy::UsageText)),
    ("Status", Some(GroupBy::Status)),
];

/// Index into GROUP_MODES of the initial grouping ('Class (device type)').
pub const DEFAULT_GROUP_MODE: usize = 1;

// ---- Records ---------------------------------------------------------------------------------------

/// How a package is signed: the "signer score" Windows stores for it (DEVPKEY_DriverPackage_SignerScore).
/// Lower is more trusted; 0x80000000 is unsigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature(pub u32);

impl Signature {
    /// The class name Windows itself gives the score (the table in drvstore.dll, same in Windows 10 and 11).
    pub fn as_str(&self) -> &'static str {
        match self.0 {
            0x0D00_0001 => "Logo Premium",
            0x0D00_0002 => "Logo Standard",
            0x0D00_0003 => "Inbox",
            0x0D00_0004 => "Unclassified",
            0x0D00_0005 => "WHQL",
            0x0F00_0000 => "Authenticode",
            0x8000_0000 => "Unsigned",
            0xC000_0000 => "Win9X Suspect",
            _ => "Unknown",
        }
    }

    /// Sort order: the least trusted first (a higher score comes first), because those are the ones worth looking at.
    fn rank(&self) -> u32 {
        u32::MAX - self.0
    }
}

/// What the Driver Store tells about one package (plus what is in its folder).
#[derive(Clone, Debug)]
pub struct RawPackage {
    pub published_name: String,
    pub number: i32,
    pub original_inf: String,
    pub folder: String,
    pub extension_id: String,
    pub provider: String,
    pub class: String,
    pub version: NetVersion,
    pub date: Date,
    pub boot_critical: bool,
    pub signature: Signature,
    /// Who signed the package ("Microsoft Windows Hardware Compatibility Publisher"); empty when not signed.
    pub signer: String,
    /// When the package was added to the Driver Store (local time); None when Windows does not say.
    pub install_date: Option<DateTime>,
    pub size_bytes: u64,
    /// False when some file or folder of the package could not be read: `size_bytes` is then a minimum.
    pub size_exact: bool,
    /// The files in the package folder (names below it), sorted.
    pub files: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Latest,
    Old,
    Review,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Latest => "Latest",
            Status::Old => "Old",
            Status::Review => "Review",
        }
    }
}

/// A record shown on screen.
#[derive(Clone, Debug)]
pub struct Driver {
    pub published_name: String,
    pub number: i32,
    pub original_inf: String,
    pub folder: String,
    pub extension_id: String,
    pub provider: String,
    pub class: String,
    pub version: NetVersion,
    pub date: Date,
    pub boot_critical: bool,
    pub signature: Signature,
    pub signer: String,
    pub install_date: Option<DateTime>,
    pub size_bytes: u64,
    pub size_exact: bool,
    pub files: Vec<String>,
    pub is_old: bool,
    pub status: Status,
    pub status_text: String,
    /// False for an offline image: there are no devices to ask, so "in use" is not known.
    pub usage_known: bool,
    pub in_use: bool,
    /// In use, but only by devices that are not plugged in right now.
    pub only_disconnected: bool,
    /// At least one device bound to the package has a problem code (Device Manager shows it as "Code NN").
    pub has_problem_device: bool,
    pub usage_text: String,
    pub in_use_text: String,
    pub device_text: String,
    /// Instance IDs of the devices bound to the package, devices that are plugged in first.
    pub device_ids: Vec<String>,
    /// The devices bound to the package that are not plugged in right now.
    pub absent_devices: Vec<DeviceRef>,
    pub checked: bool,
    /// The user protected this package: no automatic rule checks it and a removal skips it.
    pub protected: bool,
}

/// `[int](([string]$_.Driver) -replace '\D', '')`
pub fn parse_package_number(published_name: &str) -> Result<i32, String> {
    let digits: String = published_name.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Ok(0);
    }
    digits.parse::<i32>().map_err(|_| format!("Cannot convert value \"{digits}\" to type \"System.Int32\"."))
}

// ---- Devices ---------------------------------------------------------------------------------------

/// One device known to Windows (present or not) and the driver packages it uses.
#[derive(Clone, Debug, Default)]
pub struct DeviceRecord {
    pub instance_id: String,
    /// Friendly name, or the device description; may be empty.
    pub name: String,
    /// The package the device uses, as "oem16.inf" (empty when none).
    pub inf: String,
    /// Extension packages (oemNN.inf) that also apply to the device.
    pub extended_infs: Vec<String>,
    /// False when the device is not plugged in right now.
    pub present: bool,
    /// The Device Manager problem code of a device that is plugged in; 0 = no problem.
    pub problem: u32,
}

/// A device bound to a package: the text shown for it and its instance ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRef {
    pub name: String,
    pub instance_id: String,
    pub present: bool,
    pub problem: u32,
}

/// Maps each published package name ("oem16.inf", lower case) to the devices bound to it. A device counts for
/// the package it uses AND for its extension packages: an extension INF that looks unused here could be
/// in use, and removing it would change the device.
pub fn new_device_map(devices: &[DeviceRecord]) -> HashMap<String, Vec<DeviceRef>> {
    let mut map: HashMap<String, Vec<DeviceRef>> = HashMap::new();
    for device in devices {
        let mut name = if device.name.is_empty() { String::from("(unnamed device)") } else { device.name.clone() };
        if !device.present {
            name.push_str(" (not connected)");
        }
        if device.problem != 0 {
            name.push_str(&format!(" (problem code {})", device.problem));
        }
        let reference = DeviceRef { name, instance_id: device.instance_id.clone(), present: device.present, problem: device.problem };
        let mut infs: Vec<String> = Vec::new();
        for inf in std::iter::once(&device.inf).chain(device.extended_infs.iter()) {
            let key = inf.to_lowercase();
            if !key.is_empty() && !infs.contains(&key) {
                infs.push(key);
            }
        }
        for key in infs {
            map.entry(key).or_default().push(reference.clone());
        }
    }
    map
}

/// "A; B (+2 more)", or "-" when no device uses the package (Format-DeviceText).
/// Instance IDs of the devices of a package for the "Device ID" column: the first three, then "(+N more)";
/// "-" when no device uses it.
pub fn format_device_ids(ids: &[String]) -> String {
    if ids.is_empty() {
        return "-".to_string();
    }
    let shown: Vec<&str> = ids.iter().take(3).map(|s| s.as_str()).collect();
    if ids.len() > 3 {
        format!("{} (+{} more)", shown.join("; "), ids.len() - 3)
    } else {
        shown.join("; ")
    }
}

pub fn format_device_text(names: &[String]) -> String {
    // Sort-Object -Unique: culture-aware, case-insensitive; the first of equal items is kept.
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort_by(|a, b| compare_ignore_case(a, b));
    let mut unique: Vec<&String> = Vec::new();
    for item in sorted {
        if let Some(last) = unique.last() {
            if compare_ignore_case(last, item) == Ordering::Equal {
                continue;
            }
        }
        unique.push(item);
    }
    if unique.is_empty() {
        return "-".to_string();
    }
    if unique.len() <= 2 {
        return unique.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ");
    }
    format!("{} (+{} more)", [unique[0].as_str(), unique[1].as_str()].join("; "), unique.len() - 2)
}

// ---- Classification --------------------------------------------------------------------------------

fn eq_ci(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Two packages are the same driver when class, extension ID, provider and INF name match
/// (the same rule Driver Store Explorer uses).
pub fn is_same_driver(a: &RawPackage, b: &RawPackage) -> bool {
    eq_ci(&a.original_inf, &b.original_inf)
        && eq_ci(&a.class, &b.class)
        && eq_ci(&a.extension_id, &b.extension_id)
        && eq_ci(&a.provider, &b.provider)
}

/// A candidate supersedes a package when it is the same driver, its date AND version are not lower,
/// and at least one of them is higher.
pub fn is_newer_version(candidate: &RawPackage, package: &RawPackage) -> bool {
    is_same_driver(candidate, package)
        && candidate.version >= package.version
        && candidate.date >= package.date
        && (candidate.version > package.version || candidate.date > package.date)
}

/// Same driver, but date and version disagree about which package is newer.
pub fn is_conflicting(candidate: &RawPackage, package: &RawPackage) -> bool {
    is_same_driver(candidate, package)
        && ((candidate.version > package.version && candidate.date < package.date)
            || (candidate.version < package.version && candidate.date > package.date))
}

/// Identical packages: the same driver with the same version AND the same date.
fn is_identical(a: &RawPackage, b: &RawPackage) -> bool {
    is_same_driver(a, b) && a.version == b.version && a.date == b.date
}

/// Which package of a set of identical ones is kept: one that a device uses, otherwise the lowest oemNN.
/// Returns the index (into `raw`) of the package that stays.
fn kept_duplicate(raw: &[RawPackage], device_map: &HashMap<String, Vec<DeviceRef>>, package: &RawPackage) -> usize {
    let used = |p: &RawPackage| device_map.get(&p.published_name.to_lowercase()).map(|d| !d.is_empty()).unwrap_or(false);
    let mut best: Option<usize> = None;
    for (index, candidate) in raw.iter().enumerate() {
        if !is_identical(candidate, package) {
            continue;
        }
        best = Some(match best {
            None => index,
            Some(current) => {
                let (a, b) = (&raw[current], candidate);
                let better = match (used(b), used(a)) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => b.number < a.number,
                };
                if better {
                    index
                } else {
                    current
                }
            }
        });
    }
    best.expect("a package is identical to itself")
}

/// Turns raw package data into the records shown on screen.
///
/// A package is OLD when a newer package of the same driver exists, or when an identical package (same
/// version and date) exists that is kept instead: of identical packages one stays, the others are duplicates.
pub fn new_driver_records(raw: &[RawPackage], device_map: &HashMap<String, Vec<DeviceRef>>) -> Vec<Driver> {
    build_records(raw, device_map, true)
}

/// The records of an OFFLINE image: there are no devices to ask, so the usage of every package is "Unknown".
pub fn new_offline_driver_records(raw: &[RawPackage]) -> Vec<Driver> {
    build_records(raw, &HashMap::new(), false)
}

fn build_records(raw: &[RawPackage], device_map: &HashMap<String, Vec<DeviceRef>>, usage_known: bool) -> Vec<Driver> {
    raw.iter()
        .enumerate()
        .map(|(position, package)| {
            let newer: Vec<&RawPackage> = raw.iter().filter(|c| is_newer_version(c, package)).collect();
            let conflicting: Vec<&RawPackage> = raw.iter().filter(|c| is_conflicting(c, package)).collect();
            let kept = kept_duplicate(raw, device_map, package);

            let devices: Vec<DeviceRef> =
                device_map.get(&package.published_name.to_lowercase()).cloned().unwrap_or_default();
            // Devices that are plugged in first: they are the ones worth opening in Device Manager.
            let mut device_ids: Vec<(bool, String)> =
                devices.iter().map(|d| (d.present, d.instance_id.clone())).collect();
            device_ids.sort_by_key(|(present, _)| !*present);
            let device_names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();

            let is_duplicate = kept != position;
            let is_old = !newer.is_empty() || is_duplicate;
            let in_use = !devices.is_empty();
            let only_disconnected = in_use && devices.iter().all(|d| !d.present);
            let has_problem_device = devices.iter().any(|d| d.problem != 0);

            let (status, status_text) = if !newer.is_empty() {
                (
                    Status::Old,
                    format!(
                        "Old - superseded by {}",
                        newer.iter().map(|p| p.published_name.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                )
            } else if is_duplicate {
                (
                    Status::Old,
                    format!("Old - duplicate of {} (same version and date)", raw[kept].published_name),
                )
            } else if !conflicting.is_empty() {
                (
                    Status::Review,
                    format!(
                        "Review - version and date disagree with {}",
                        conflicting.iter().map(|p| p.published_name.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                )
            } else {
                (Status::Latest, "Latest".to_string())
            };

            Driver {
                published_name: package.published_name.clone(),
                number: package.number,
                original_inf: package.original_inf.clone(),
                folder: package.folder.clone(),
                extension_id: package.extension_id.clone(),
                provider: package.provider.clone(),
                class: package.class.clone(),
                version: package.version,
                date: package.date,
                boot_critical: package.boot_critical,
                signature: package.signature,
                signer: package.signer.clone(),
                install_date: package.install_date,
                size_bytes: package.size_bytes,
                size_exact: package.size_exact,
                files: package.files.clone(),
                is_old,
                status,
                status_text,
                usage_known,
                in_use,
                only_disconnected,
                has_problem_device,
                usage_text: if !usage_known { "Unknown" } else if in_use { "In use" } else { "Unused" }.to_string(),
                in_use_text: if !usage_known {
                    "Unknown".to_string()
                } else if in_use {
                    format!("Yes ({})", device_names.len())
                } else {
                    "No".to_string()
                },
                device_text: if usage_known { format_device_text(&device_names) } else { "Unknown".to_string() },
                device_ids: device_ids.into_iter().map(|(_, id)| id).collect(),
                absent_devices: devices.iter().filter(|d| !d.present).cloned().collect(),
                checked: false,
                protected: false,
            }
        })
        .collect()
}

/// What identifies a package in the protected list: published name, INF name and version, lower case. The
/// published name alone is not enough: Windows reuses the oemNN numbers, so a new package could inherit the
/// protection of one that was removed.
pub fn protection_key(package: &Driver) -> String {
    format!("{}|{}|{}", package.published_name, package.original_inf, package.version).to_lowercase()
}

/// The text of the Status column: the status, plus a mark when the package is protected.
pub fn status_cell(package: &Driver) -> String {
    if package.protected {
        format!("{} (protected)", package.status_text)
    } else {
        package.status_text.clone()
    }
}

/// True for the packages the automatic rules never check (see NEVER_AUTO_SELECT_INFS), and for protected ones.
pub fn is_never_auto_selected(package: &Driver) -> bool {
    package.protected
        || NEVER_AUTO_SELECT_INFS.iter().any(|name| eq_ci(&package.original_inf, name))
}

/// "Check old packages" rule: only old (superseded or duplicate) packages; in-use and boot-critical ones only
/// when the user opted in; never the packages in NEVER_AUTO_SELECT_INFS.
pub fn is_auto_selectable(package: &Driver, include_in_use: bool, include_boot_critical: bool) -> bool {
    if !package.is_old || is_never_auto_selected(package) {
        return false;
    }
    if package.in_use && !include_in_use {
        return false;
    }
    if package.boot_critical && !include_boot_critical {
        return false;
    }
    true
}

/// "Check unused packages" rule: no device is bound to the package, whatever its age. Boot-critical packages
/// only when the user opted in; never the packages in NEVER_AUTO_SELECT_INFS.
pub fn is_unused_selectable(package: &Driver, include_boot_critical: bool) -> bool {
    !package.in_use && !is_never_auto_selected(package) && (include_boot_critical || !package.boot_critical)
}

/// True when the loaded list still contains exactly this package (same oemNN number, INF name, version and date).
pub fn is_package_listed(package: &Driver, list: &[Driver]) -> bool {
    list.iter().any(|d| {
        eq_ci(&d.published_name, &package.published_name)
            && eq_ci(&d.original_inf, &package.original_inf)
            && d.version == package.version
            && d.date == package.date
    })
}

// ---- Add only newer packages ------------------------------------------------------------------------

/// One .inf file of the folder chosen in "Add only newer driver packages", as DISM read it.
#[derive(Clone, Debug)]
pub struct InfCandidate {
    /// Full path of the .inf file.
    pub path: String,
    /// File name of the .inf ("netrtle.inf").
    pub original_inf: String,
    pub class: String,
    pub provider: String,
    pub version: NetVersion,
    pub date: Date,
}

/// What to do with one .inf file of the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddDecision {
    /// Newer than what is installed (or nothing like it is installed): add it.
    Add,
    /// Not newer: not added. The text says why.
    Skip(String),
    /// The rules cannot decide: the user is asked. The text says why.
    Ask(String),
}

fn candidate_as_raw(candidate: &InfCandidate) -> RawPackage {
    RawPackage {
        published_name: candidate.path.clone(),
        number: 0,
        original_inf: candidate.original_inf.clone(),
        folder: String::new(),
        // DISM does not give the extension ID of an .inf file: an extension package never matches one here.
        extension_id: String::new(),
        provider: candidate.provider.clone(),
        class: candidate.class.clone(),
        version: candidate.version,
        date: candidate.date,
        boot_critical: false,
        signature: Signature(0),
        signer: String::new(),
        install_date: None,
        size_bytes: 0,
        size_exact: true,
        files: Vec::new(),
    }
}

fn driver_as_raw(package: &Driver) -> RawPackage {
    RawPackage {
        published_name: package.published_name.clone(),
        number: package.number,
        original_inf: package.original_inf.clone(),
        folder: package.folder.clone(),
        extension_id: package.extension_id.clone(),
        provider: package.provider.clone(),
        class: package.class.clone(),
        version: package.version,
        date: package.date,
        boot_critical: package.boot_critical,
        signature: package.signature,
        signer: package.signer.clone(),
        install_date: package.install_date,
        size_bytes: package.size_bytes,
        size_exact: package.size_exact,
        files: Vec::new(),
    }
}

fn describe_raw(package: &RawPackage) -> String {
    format!("{} {} ({})", package.published_name, package.version, format_date(package.date))
}

fn is_identical_version(a: &RawPackage, b: &RawPackage) -> bool {
    a.version == b.version && a.date == b.date
}

/// Decides, with the same rules that mark packages as old in the list, which .inf files of a folder to add.
/// `candidates` has one entry per file, in order: what DISM read, or why it could not read it.
///  1. Compared with the installed packages of the same driver (see `is_same_driver`): an installed package
///     that is newer, or has the same version and date -> Skip; version and date that disagree -> Ask;
///     otherwise Add. A file DISM could not read, or that has no provider, is also Ask.
///  2. Among the files still marked Add, a file superseded by another one of them is Skip, and of identical
///     files only the first stays.
pub fn decide_additions(installed: &[Driver], candidates: &[Result<InfCandidate, String>]) -> Vec<AddDecision> {
    let installed: Vec<RawPackage> = installed.iter().map(driver_as_raw).collect();
    let raws: Vec<Option<RawPackage>> = candidates.iter().map(|c| c.as_ref().ok().map(candidate_as_raw)).collect();

    let mut decisions: Vec<AddDecision> = candidates
        .iter()
        .zip(&raws)
        .map(|(candidate, raw)| {
            let (Ok(candidate), Some(raw)) = (candidate, raw) else {
                let reason = candidate.as_ref().err().cloned().unwrap_or_default();
                return AddDecision::Ask(format!("DISM could not read the file: {reason}"));
            };
            if candidate.provider.is_empty() {
                return AddDecision::Ask("DISM gave no provider, so it cannot be compared with the installed packages".to_string());
            }
            let same: Vec<&RawPackage> = installed.iter().filter(|p| is_same_driver(raw, p)).collect();
            if let Some(newer) = same.iter().find(|p| is_newer_version(p, raw)) {
                return AddDecision::Skip(format!("superseded by installed {}", describe_raw(newer)));
            }
            if let Some(equal) = same.iter().find(|p| is_identical_version(p, raw)) {
                return AddDecision::Skip(format!("same version and date already installed as {}", equal.published_name));
            }
            if let Some(other) = same.iter().find(|p| is_conflicting(raw, p)) {
                return AddDecision::Ask(format!("version and date disagree with installed {}", describe_raw(other)));
            }
            AddDecision::Add
        })
        .collect();

    // Among the files that are still to be added.
    let to_add: Vec<usize> = (0..decisions.len()).filter(|&i| decisions[i] == AddDecision::Add).collect();
    let mut updates: Vec<(usize, AddDecision)> = Vec::new();
    for &i in &to_add {
        let Some(this) = &raws[i] else { continue };
        for &j in &to_add {
            let Some(other) = &raws[j] else { continue };
            if i == j || !is_same_driver(this, other) {
                continue;
            }
            if is_newer_version(other, this) {
                updates.push((i, AddDecision::Skip(format!("superseded by {} in the same folder", describe_raw(other)))));
                break;
            }
            if is_identical_version(this, other) && j < i {
                updates.push((i, AddDecision::Skip(format!("same version and date as {} in the same folder", other.published_name))));
                break;
            }
        }
    }
    for (index, decision) in updates {
        decisions[index] = decision;
    }
    decisions
}

// ---- List view -------------------------------------------------------------------------------------

/// Text searched by the filter box (Get-SearchText).
pub fn search_text(package: &Driver) -> String {
    [
        package.published_name.as_str(),
        package.original_inf.as_str(),
        package.provider.as_str(),
        package.class.as_str(),
        &package.version.to_string(),
        &format_date(package.date),
        package.signature.as_str(),
        package.signer.as_str(),
        package.extension_id.as_str(),
        &format_install_date(package.install_date),
        package.in_use_text.as_str(),
        package.device_text.as_str(),
        &format_device_ids(&package.device_ids),
        &status_cell(package),
        &format_files(&package.files),
        package.folder.as_str(),
    ]
    .join(" ")
}

/// Row background: light blue = old and unused, light yellow = old and in use, light gray = review,
/// none = latest (Get-RowColor).
pub fn row_color(package: &Driver) -> Option<(u8, u8, u8)> {
    if package.status == Status::Review {
        return Some(COLOR_REVIEW);
    }
    if package.is_old && package.in_use {
        return Some(COLOR_OLD_IN_USE);
    }
    if package.is_old {
        return Some(COLOR_OLD_UNUSED);
    }
    None
}

fn compare_by(sort_by: SortBy, a: &Driver, b: &Driver) -> Ordering {
    match sort_by {
        SortBy::Number => a.number.cmp(&b.number),
        SortBy::OriginalInf => compare_ignore_case(&a.original_inf, &b.original_inf),
        SortBy::Provider => compare_ignore_case(&a.provider, &b.provider),
        SortBy::Class => compare_ignore_case(&a.class, &b.class),
        SortBy::Version => a.version.cmp(&b.version),
        SortBy::Date => a.date.cmp(&b.date),
        SortBy::InstallDate => a.install_date.cmp(&b.install_date),
        SortBy::SizeBytes => a.size_bytes.cmp(&b.size_bytes),
        SortBy::FileCount => a.files.len().cmp(&b.files.len()),
        SortBy::BootCritical => a.boot_critical.cmp(&b.boot_critical),
        SortBy::Signature => a.signature.rank().cmp(&b.signature.rank()),
        SortBy::Signer => compare_ignore_case(&a.signer, &b.signer),
        SortBy::ExtensionId => compare_ignore_case(&a.extension_id, &b.extension_id),
        SortBy::InUse => a.in_use.cmp(&b.in_use),
        SortBy::DeviceText => compare_ignore_case(&a.device_text, &b.device_text),
        SortBy::DeviceId => compare_ignore_case(&format_device_ids(&a.device_ids), &format_device_ids(&b.device_ids)),
        SortBy::Status => compare_ignore_case(a.status.as_str(), b.status.as_str()),
        SortBy::Folder => compare_ignore_case(&a.folder, &b.folder),
    }
}

fn group_value(group_by: GroupBy, package: &Driver) -> &str {
    match group_by {
        GroupBy::Class => &package.class,
        GroupBy::Provider => &package.provider,
        GroupBy::OriginalInf => &package.original_inf,
        GroupBy::UsageText => &package.usage_text,
        GroupBy::Status => package.status.as_str(),
    }
}

/// Packages that pass the filter box and "Show only old packages", in the selected sort order
/// (Get-VisiblePackages). Returns indexes into `drivers`.
pub fn visible_packages(
    drivers: &[Driver],
    filter_text: &str,
    old_only: bool,
    disconnected_only: bool,
    problem_only: bool,
    sort_column: usize,
    sort_descending: bool,
) -> Vec<usize> {
    let filter = filter_text.trim();
    let mut indexes: Vec<usize> = (0..drivers.len()).collect();
    if !filter.is_empty() {
        indexes.retain(|&i| contains_ordinal_ignore_case(&search_text(&drivers[i]), filter));
    }
    if old_only {
        indexes.retain(|&i| drivers[i].is_old);
    }
    if disconnected_only {
        indexes.retain(|&i| drivers[i].only_disconnected);
    }
    if problem_only {
        indexes.retain(|&i| drivers[i].has_problem_device);
    }
    let sort_by = COLUMN_DEFINITIONS[sort_column].sort_by;
    // Sort-Object -Property @{Expression = $sortBy; Descending = ...}, @{Expression = 'Number'}
    indexes.sort_by(|&x, &y| {
        let primary = compare_by(sort_by, &drivers[x], &drivers[y]);
        let primary = if sort_descending { primary.reverse() } else { primary };
        primary.then_with(|| drivers[x].number.cmp(&drivers[y].number))
    });
    indexes
}

/// One row of the list as it is inserted into the ListView.
#[derive(Clone, Debug)]
pub struct ViewRow {
    pub driver: usize,
    pub group: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ViewModel {
    pub rows: Vec<ViewRow>,
    pub group_headers: Vec<String>,
}

/// Groups the visible packages: Group-Object -Property X | Sort-Object -Property Name (Update-View).
pub fn build_view(drivers: &[Driver], visible: &[usize], group_by: Option<GroupBy>) -> ViewModel {
    let mut model = ViewModel::default();
    let Some(group_by) = group_by else {
        model.rows = visible.iter().map(|&i| ViewRow { driver: i, group: None }).collect();
        return model;
    };

    // Group-Object: groups in order of first appearance, values compared case-insensitively;
    // the group name is the first value seen.
    let mut names: Vec<String> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut lookup: HashMap<String, usize> = HashMap::new();
    for &i in visible {
        let value = group_value(group_by, &drivers[i]);
        let key = value.to_lowercase();
        let slot = *lookup.entry(key).or_insert_with(|| {
            names.push(value.to_string());
            members.push(Vec::new());
            names.len() - 1
        });
        members[slot].push(i);
    }
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by(|&a, &b| compare_ignore_case(&names[a], &names[b]));

    for slot in order {
        let title = if names[slot].is_empty() { "(none)".to_string() } else { names[slot].clone() };
        let old_count = members[slot].iter().filter(|&&i| drivers[i].is_old).count();
        let bytes: u64 = members[slot].iter().map(|&i| drivers[i].size_bytes).sum();
        let header = format!(
            "{}  -  {} package(s), {} old, {}",
            title,
            members[slot].len(),
            old_count,
            format_size(bytes as f64)
        );
        let group_index = model.group_headers.len();
        model.group_headers.push(header);
        for &i in &members[slot] {
            model.rows.push(ViewRow { driver: i, group: Some(group_index) });
        }
    }
    model
}

/// "1.2 MB", or "1.2 MB+" when part of the package could not be read (the real size is larger).
pub fn format_size_text(bytes: u64, exact: bool) -> String {
    let text = format_size(bytes as f64);
    if exact {
        text
    } else {
        format!("{text}+")
    }
}

/// "-" when Windows gives no install date, otherwise "2026-09-03 14:30".
pub fn format_install_date(install_date: Option<DateTime>) -> String {
    install_date.map(format_date_time).unwrap_or_else(|| "-".to_string())
}

/// How many files the "Driver files" column names before it says "and N more files".
const FILES_SHOWN: usize = 5;

/// "3 files: a.cat, a.inf, a.sys" or "12 files: a, b, c, d, e and 7 more files". The names are sorted;
/// the full list is the package folder ("Open package folder" in the right-click menu).
pub fn format_files(files: &[String]) -> String {
    if files.is_empty() {
        return "-".to_string();
    }
    let shown = files.iter().take(FILES_SHOWN).map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
    let total = if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) };
    match files.len() - files.len().min(FILES_SHOWN) {
        0 => format!("{total}: {shown}"),
        1 => format!("{total}: {shown} and 1 more file"),
        more => format!("{total}: {shown} and {more} more files"),
    }
}

/// The eighteen text sub-items of a row (one per column).
pub fn row_texts(package: &Driver) -> [String; 18] {
    [
        package.published_name.clone(),
        package.original_inf.clone(),
        package.provider.clone(),
        package.class.clone(),
        package.version.to_string(),
        format_date(package.date),
        format_install_date(package.install_date),
        package.signature.as_str().to_string(),
        package.signer.clone(),
        package.extension_id.clone(),
        if package.boot_critical { "Yes" } else { "No" }.to_string(),
        package.in_use_text.clone(),
        status_cell(package),
        package.device_text.clone(),
        format_device_ids(&package.device_ids),
        format_size_text(package.size_bytes, package.size_exact),
        format_files(&package.files),
        package.folder.clone(),
    ]
}

/// Status bar summary (Update-Summary). Says the same thing as the row colors, in plain text.
pub fn summary_text(drivers: &[Driver], shown: usize) -> String {
    let total = drivers.len();
    let old: Vec<&Driver> = drivers.iter().filter(|d| d.is_old).collect();
    let old_in_use = old.iter().filter(|d| d.in_use).count();
    let old_boot = old.iter().filter(|d| d.boot_critical).count();
    let review = drivers.iter().filter(|d| d.status == Status::Review).count();
    let checked: Vec<&Driver> = drivers.iter().filter(|d| d.checked).collect();
    let bytes: u64 = checked.iter().map(|d| d.size_bytes).sum();
    if !drivers.is_empty() && drivers.iter().all(|d| !d.usage_known) {
        return format!(
            "OFFLINE IMAGE | {} packages | {} old ({} boot-critical) | {} to review | {} shown | {} checked ({}) | Device usage is unknown offline | Gray = review",
            total,
            old.len(),
            old_boot,
            review,
            shown,
            checked.len(),
            format_size(bytes as f64)
        );
    }
    format!(
        "{} packages | {} old ({} unused, {} in use; {} boot-critical) | {} to review | {} shown | {} checked ({}) | Light blue = old, unused | Light yellow = old, in use | Gray = review",
        total,
        old.len(),
        old.len() - old_in_use,
        old_in_use,
        old_boot,
        review,
        shown,
        checked.len(),
        format_size(bytes as f64)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, inf: &str, ver: &str, date: (i32, u32, u32)) -> RawPackage {
        RawPackage {
            published_name: name.into(),
            number: parse_package_number(name).unwrap(),
            original_inf: inf.into(),
            folder: format!("C:\\Windows\\System32\\DriverStore\\FileRepository\\{inf}_x"),
            extension_id: String::new(),
            provider: "Intel".into(),
            class: "Net".into(),
            version: NetVersion::parse(ver).unwrap(),
            date: Date::from_ymd_opt(date.0, date.1, date.2).unwrap(),
            boot_critical: false,
            signature: Signature(0x0D00_0005),
            signer: "Microsoft Windows Hardware Compatibility Publisher".into(),
            install_date: None,
            size_bytes: 1000,
            size_exact: true,
            files: Vec::new(),
        }
    }

    fn device(name: &str, inf: &str, extended: &[&str], present: bool, id: &str) -> DeviceRecord {
        DeviceRecord {
            instance_id: id.into(),
            name: name.into(),
            inf: inf.into(),
            extended_infs: extended.iter().map(|s| s.to_string()).collect(),
            present,
            problem: 0,
        }
    }

    fn map_of(devices: &[DeviceRecord]) -> HashMap<String, Vec<DeviceRef>> {
        new_device_map(devices)
    }

    #[test]
    fn old_review_latest_and_duplicates() {
        let packages = vec![
            raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem2.inf", "a.inf", "2.0.0.0", (2021, 1, 1)), // newest of a.inf
            raw("oem3.inf", "b.inf", "3.0.0.0", (2020, 1, 1)),
            raw("oem4.inf", "b.inf", "2.0.0.0", (2021, 1, 1)), // higher date, lower version -> review both
            raw("oem5.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem6.inf", "c.inf", "1.0.0.0", (2020, 1, 1)), // identical to oem5: oem5 stays, oem6 is a duplicate
        ];
        let devices = map_of(&[device("Wi-Fi", "oem1.inf", &[], true, "PCI\\A")]);
        let drivers = new_driver_records(&packages, &devices);

        assert_eq!(drivers[0].status, Status::Old);
        assert_eq!(drivers[0].status_text, "Old - superseded by oem2.inf");
        assert!(drivers[0].in_use);
        assert_eq!(drivers[0].in_use_text, "Yes (1)");
        assert_eq!(drivers[0].usage_text, "In use");
        assert_eq!(drivers[0].device_ids, vec!["PCI\\A".to_string()]);
        assert_eq!(drivers[1].status, Status::Latest);
        assert_eq!(drivers[2].status, Status::Review);
        assert_eq!(drivers[2].status_text, "Review - version and date disagree with oem4.inf");
        assert_eq!(drivers[3].status, Status::Review);
        assert_eq!(drivers[4].status, Status::Latest);
        assert!(!drivers[4].is_old);
        assert_eq!(drivers[5].status, Status::Old);
        assert!(drivers[5].is_old);
        assert_eq!(drivers[5].status_text, "Old - duplicate of oem5.inf (same version and date)");
        assert_eq!(drivers[4].device_text, "-");
        assert_eq!(drivers[4].in_use_text, "No");

        assert_eq!(row_color(&drivers[0]), Some(COLOR_OLD_IN_USE));
        assert_eq!(row_color(&drivers[2]), Some(COLOR_REVIEW));
        assert_eq!(row_color(&drivers[5]), Some(COLOR_OLD_UNUSED));
        assert_eq!(row_color(&drivers[1]), None);

        assert!(!is_auto_selectable(&drivers[0], false, false)); // in use
        assert!(is_auto_selectable(&drivers[0], true, false));
        assert!(!is_auto_selectable(&drivers[1], true, true)); // not old
        assert!(is_auto_selectable(&drivers[5], false, false)); // unused duplicate
    }

    #[test]
    fn the_duplicate_that_a_device_uses_is_the_one_kept() {
        let packages = vec![
            raw("oem5.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem6.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem9.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
        ];
        let devices = map_of(&[device("Card", "oem6.inf", &[], true, "PCI\\B")]);
        let drivers = new_driver_records(&packages, &devices);
        assert!(drivers[0].is_old);
        assert!(!drivers[1].is_old);
        assert!(drivers[2].is_old);
        assert_eq!(drivers[0].status_text, "Old - duplicate of oem6.inf (same version and date)");
        // Different version or different INF: never a duplicate.
        let packages = vec![raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)), raw("oem2.inf", "z.inf", "1.0.0.0", (2020, 1, 1))];
        assert!(new_driver_records(&packages, &HashMap::new()).iter().all(|d| !d.is_old));
    }

    #[test]
    fn version_four_parts_vs_two() {
        // [version]"1.2" is lower than [version]"1.2.0.0" in .NET.
        let a = raw("oem1.inf", "a.inf", "1.2", (2020, 1, 1));
        let b = raw("oem2.inf", "a.inf", "1.2.0.0", (2020, 1, 1));
        assert!(is_newer_version(&b, &a));
        assert!(!is_newer_version(&a, &b));
    }

    #[test]
    fn device_text() {
        assert_eq!(format_device_text(&[]), "-");
        assert_eq!(format_device_text(&["B".into(), "a".into()]), "a; B");
        assert_eq!(format_device_text(&["x".into(), "X".into()]), "x");
        assert_eq!(format_device_text(&["c".into(), "b".into(), "a".into(), "d".into()]), "a; b (+2 more)");
    }

    #[test]
    fn device_map_with_extension_infs() {
        let devices = vec![
            device("Card", "OEM1.INF", &["oem7.inf", "OEM1.inf"], true, "PCI\\A"),
            device("", "oem1.inf", &[], false, "PCI\\B"),
            device("No driver", "", &[], true, "PCI\\C"),
            device("Only extension", "", &["oem9.inf"], true, "PCI\\D"),
        ];
        let map = new_device_map(&devices);
        let names = |key: &str| map[key].iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("oem1.inf"), vec!["Card".to_string(), "(unnamed device) (not connected)".to_string()]);
        assert_eq!(names("oem7.inf"), vec!["Card".to_string()]); // extension package: in use too
        assert_eq!(names("oem9.inf"), vec!["Only extension".to_string()]);
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn present_devices_come_first() {
        let packages = vec![raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1))];
        let devices = map_of(&[
            device("Gone", "oem1.inf", &[], false, "USB\\GONE"),
            device("Here", "oem1.inf", &[], true, "USB\\HERE"),
        ]);
        let drivers = new_driver_records(&packages, &devices);
        assert_eq!(drivers[0].device_ids, vec!["USB\\HERE".to_string(), "USB\\GONE".to_string()]);
        let absent: Vec<&str> = drivers[0].absent_devices.iter().map(|d| d.instance_id.as_str()).collect();
        assert_eq!(absent, vec!["USB\\GONE"]);
        assert_eq!(drivers[0].in_use_text, "Yes (2)");
    }

    #[test]
    fn never_auto_selected_and_unused_rule() {
        let mut packages = vec![
            raw("oem1.inf", "ntprint.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem2.inf", "ntprint.inf", "2.0.0.0", (2021, 1, 1)),
            raw("oem3.inf", "x.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem4.inf", "y.inf", "1.0.0.0", (2020, 1, 1)),
        ];
        packages[2].boot_critical = true;
        let drivers = new_driver_records(&packages, &HashMap::new());
        assert!(drivers[0].is_old);
        assert!(!is_auto_selectable(&drivers[0], true, true)); // ntprint.inf is never checked automatically
        assert!(!is_unused_selectable(&drivers[0], true));
        assert!(!is_unused_selectable(&drivers[1], true)); // same INF name: protected too
        assert!(!is_unused_selectable(&drivers[2], false)); // boot-critical
        assert!(is_unused_selectable(&drivers[2], true));
        assert!(is_unused_selectable(&drivers[3], false)); // latest, but no device uses it
    }

    #[test]
    fn path_column_is_shown_sorted_and_searched() {
        let packages = vec![raw("oem1.inf", "b.inf", "1.0.0.0", (2020, 1, 1)), raw("oem2.inf", "a.inf", "1.0.0.0", (2020, 1, 1))];
        let drivers = new_driver_records(&packages, &HashMap::new());
        let texts = row_texts(&drivers[0]);
        assert_eq!(texts.len(), COLUMN_DEFINITIONS.len());
        assert_eq!(texts[17], drivers[0].folder);
        assert_eq!(COLUMN_DEFINITIONS[17].title, "Driver path");
        // Sorted by path: a.inf_x comes before b.inf_x. Found by a part of the path.
        assert_eq!(visible_packages(&drivers, "", false, false, false, 17, false), vec![1, 0]);
        assert_eq!(visible_packages(&drivers, "a.inf_x", false, false, false, 0, false), vec![1]);
    }

    fn candidate(path: &str, inf: &str, ver: &str, date: (i32, u32, u32)) -> Result<InfCandidate, String> {
        Ok(InfCandidate {
            path: path.into(),
            original_inf: inf.into(),
            class: "Net".into(),
            provider: "Intel".into(),
            version: NetVersion::parse(ver).unwrap(),
            date: Date::from_ymd_opt(date.0, date.1, date.2).unwrap(),
        })
    }

    #[test]
    fn additions_follow_the_rules_of_the_list() {
        let packages = vec![raw("oem1.inf", "a.inf", "2.0.0.0", (2021, 1, 1))];
        let installed = new_driver_records(&packages, &HashMap::new());
        let candidates = vec![
            candidate("old\\a.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),   // older than installed
            candidate("same\\a.inf", "a.inf", "2.0.0.0", (2021, 1, 1)),  // same version and date
            candidate("new\\a.inf", "a.inf", "3.0.0.0", (2022, 1, 1)),   // newer
            candidate("other\\b.inf", "b.inf", "1.0.0.0", (2019, 1, 1)), // nothing like it is installed
            candidate("mixed\\a.inf", "a.inf", "3.0.0.0", (2019, 1, 1)), // version up, date down
            Err("The system cannot find the file".to_string()),
        ];
        let decisions = decide_additions(&installed, &candidates);
        assert!(matches!(&decisions[0], AddDecision::Skip(r) if r.contains("superseded by installed oem1.inf")));
        assert!(matches!(&decisions[1], AddDecision::Skip(r) if r.contains("already installed as oem1.inf")));
        // "new" is also superseded by nothing installed; it is the newest of the folder too
        assert_eq!(decisions[2], AddDecision::Add);
        assert_eq!(decisions[3], AddDecision::Add);
        assert!(matches!(&decisions[4], AddDecision::Ask(r) if r.contains("disagree")));
        assert!(matches!(&decisions[5], AddDecision::Ask(r) if r.contains("could not read")));
    }

    #[test]
    fn additions_inside_the_folder_keep_only_the_newest_and_one_of_identical_files() {
        let installed = new_driver_records(&[], &HashMap::new());
        let candidates = vec![
            candidate("v1\\a.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),
            candidate("v2\\a.inf", "a.inf", "2.0.0.0", (2021, 1, 1)),
            candidate("v2copy\\a.inf", "a.inf", "2.0.0.0", (2021, 1, 1)),
        ];
        let decisions = decide_additions(&installed, &candidates);
        assert!(matches!(&decisions[0], AddDecision::Skip(r) if r.contains("superseded by")));
        assert_eq!(decisions[1], AddDecision::Add);
        assert!(matches!(&decisions[2], AddDecision::Skip(r) if r.contains("same version and date")));
    }

    #[test]
    fn a_file_without_provider_is_asked_about() {
        let installed = new_driver_records(&[], &HashMap::new());
        let mut c = candidate("x\\a.inf", "a.inf", "1.0.0.0", (2020, 1, 1)).unwrap();
        c.provider.clear();
        assert!(matches!(&decide_additions(&installed, &[Ok(c)])[0], AddDecision::Ask(r) if r.contains("no provider")));
    }

    #[test]
    fn protected_package_is_marked_and_never_auto_selected() {
        let packages = vec![raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)), raw("oem2.inf", "a.inf", "2.0.0.0", (2021, 1, 1))];
        let mut drivers = new_driver_records(&packages, &HashMap::new());
        assert!(is_auto_selectable(&drivers[0], true, true)); // old
        assert_eq!(protection_key(&drivers[0]), "oem1.inf|a.inf|1.0.0.0");
        drivers[0].protected = true;
        assert!(!is_auto_selectable(&drivers[0], true, true));
        assert!(!is_unused_selectable(&drivers[0], true));
        assert!(row_texts(&drivers[0])[12].ends_with(" (protected)"));
        assert!(!row_texts(&drivers[1])[12].contains("protected"));
        assert_eq!(visible_packages(&drivers, "protected", false, false, false, 0, false), vec![0]);
    }

    #[test]
    fn date_column_is_searched() {
        let packages = vec![raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)), raw("oem2.inf", "b.inf", "1.0.0.0", (2021, 6, 15))];
        let drivers = new_driver_records(&packages, &HashMap::new());
        // The text of the Date column, exactly as the list shows it.
        assert_eq!(row_texts(&drivers[1])[5], "2021-06-15");
        assert_eq!(visible_packages(&drivers, "2021-06-15", false, false, false, 0, false), vec![1]);
        assert_eq!(visible_packages(&drivers, "2020-01-01", false, false, false, 0, false), vec![0]);
    }

    #[test]
    fn signature_column_and_disconnected_filter() {
        // The class names Windows gives the signer scores.
        let name = |score: u32| Signature(score).as_str();
        assert_eq!(name(0x0D00_0001), "Logo Premium");
        assert_eq!(name(0x0D00_0002), "Logo Standard");
        assert_eq!(name(0x0D00_0003), "Inbox");
        assert_eq!(name(0x0D00_0004), "Unclassified");
        assert_eq!(name(0x0D00_0005), "WHQL");
        assert_eq!(name(0x0F00_0000), "Authenticode");
        assert_eq!(name(0x8000_0000), "Unsigned");
        assert_eq!(name(0xC000_0000), "Win9X Suspect");
        assert_eq!(name(0x1234_5678), "Unknown");

        let mut packages = vec![
            raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem2.inf", "b.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem3.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem4.inf", "d.inf", "1.0.0.0", (2020, 1, 1)),
        ];
        packages[1].signature = Signature(0x8000_0000); // Unsigned
        packages[2].signature = Signature(0x0F00_0000); // Authenticode
        let devices = map_of(&[
            device("Gone", "oem1.inf", &[], false, "USB\\GONE"),
            device("Here", "oem3.inf", &[], true, "USB\\HERE"),
            device("Gone too", "oem3.inf", &[], false, "USB\\GONE2"),
        ]);
        let drivers = new_driver_records(&packages, &devices);
        assert!(drivers[0].only_disconnected); // every device that uses it is unplugged
        assert!(!drivers[1].only_disconnected); // no device at all: unused, not "only disconnected"
        assert!(!drivers[2].only_disconnected); // one device is plugged in
        assert_eq!(visible_packages(&drivers, "", false, true, false, 0, false), vec![0]);
        assert_eq!(visible_packages(&drivers, "", false, false, false, 0, false), vec![0, 1, 2, 3]);

        // The Signature column: the class name, sort (least trusted first), search.
        assert_eq!(row_texts(&drivers[1])[7], "Unsigned");
        assert_eq!(COLUMN_DEFINITIONS[7].title, "Signature");
        assert_eq!(visible_packages(&drivers, "", false, false, false, 7, false), vec![1, 2, 0, 3]);
        assert_eq!(visible_packages(&drivers, "unsigned", false, false, false, 0, false), vec![1]);
    }

    #[test]
    fn problem_devices_filter() {
        let packages = vec![
            raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem2.inf", "b.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem3.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
        ];
        let mut broken = device("Broken", "oem1.inf", &[], true, "USB\\BAD");
        broken.problem = 28;
        let fine = device("Fine", "oem2.inf", &[], true, "USB\\OK");
        let drivers = new_driver_records(&packages, &map_of(&[broken, fine]));
        assert!(drivers[0].has_problem_device);
        assert!(!drivers[1].has_problem_device); // its device works
        assert!(!drivers[2].has_problem_device); // no device at all
        assert_eq!(drivers[0].device_text, "Broken (problem code 28)");
        assert_eq!(visible_packages(&drivers, "", false, false, true, 0, false), vec![0]);
        assert_eq!(visible_packages(&drivers, "", false, false, false, 0, false), vec![0, 1, 2]);
        // An offline image has no devices: nothing has a problem.
        assert!(new_offline_driver_records(&packages).iter().all(|d| !d.has_problem_device));
    }

    #[test]
    fn device_id_column_and_offline_records() {
        assert_eq!(format_device_ids(&[]), "-");
        let ids: Vec<String> = (1..=5).map(|i| format!("PCI\\{i}")).collect();
        assert_eq!(format_device_ids(&ids[..2]), "PCI\\1; PCI\\2");
        assert_eq!(format_device_ids(&ids), "PCI\\1; PCI\\2; PCI\\3 (+2 more)");

        let packages = vec![raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)), raw("oem2.inf", "a.inf", "2.0.0.0", (2021, 1, 1))];
        let devices = map_of(&[device("Wi-Fi", "oem1.inf", &[], true, "PCI\\A")]);
        let online = new_driver_records(&packages, &devices);
        assert_eq!(row_texts(&online[0])[14], "PCI\\A");
        assert_eq!(COLUMN_DEFINITIONS[14].title, "Device ID");
        assert_eq!(visible_packages(&online, "pci\\a", false, false, false, 0, false), vec![0]);
        assert!(online.iter().all(|d| d.usage_known));

        // Offline: usage is unknown, nothing counts as in use, the old/latest rules still work.
        let offline = new_offline_driver_records(&packages);
        assert!(offline.iter().all(|d| !d.usage_known && !d.in_use && !d.only_disconnected));
        assert_eq!(row_texts(&offline[0])[7], "WHQL");
        assert_eq!(offline[0].usage_text, "Unknown");
        assert_eq!(offline[0].in_use_text, "Unknown");
        assert_eq!(offline[0].device_text, "Unknown");
        assert!(offline[0].is_old && !offline[1].is_old);
        assert!(summary_text(&offline, 2).starts_with("OFFLINE IMAGE | 2 packages | 1 old (0 boot-critical)"));
        assert!(!summary_text(&online, 2).contains("OFFLINE"));
    }

    #[test]
    fn the_new_columns() {
        let titles: Vec<&str> = COLUMN_DEFINITIONS.iter().map(|c| c.title).collect();
        assert_eq!(
            titles,
            [
                "Published name", "Original INF", "Provider", "Class", "Version", "Date", "Install date (UTC)", "Signature", "Signer",
                "Extension ID", "Boot-critical", "In use", "Status", "Devices", "Device ID", "Size", "Driver files", "Driver path"
            ]
        );

        let mut packages = vec![
            raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem2.inf", "b.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem3.inf", "c.inf", "1.0.0.0", (2020, 1, 1)),
        ];
        packages[0].install_date = DateTime::from_filetime((1_709_677_800u64 + 11_644_473_600) * 10_000_000);
        packages[0].files = vec!["a.cat".into(), "a.inf".into()];
        packages[0].extension_id = "{abcdef12-3456-7890-abcd-ef1234567890}".into();
        packages[1].install_date = DateTime::from_filetime((1_609_459_200u64 + 11_644_473_600) * 10_000_000);
        packages[1].signer = "Acme".into();
        packages[1].files = (1..=9).map(|i| format!("f{i}.sys")).collect();
        let drivers = new_driver_records(&packages, &HashMap::new());

        let texts = row_texts(&drivers[0]);
        assert_eq!(texts[6], "2024-03-05 22:30");
        assert_eq!(texts[8], "Microsoft Windows Hardware Compatibility Publisher");
        assert_eq!(texts[9], "{abcdef12-3456-7890-abcd-ef1234567890}");
        assert_eq!(texts[16], "2 files: a.cat, a.inf");
        assert_eq!(row_texts(&drivers[2])[6], "-"); // no install date
        assert_eq!(row_texts(&drivers[2])[16], "-"); // no files

        // Sorting: install date (none first), file count, signer, extension ID.
        assert_eq!(visible_packages(&drivers, "", false, false, false, 6, false), vec![2, 1, 0]);
        assert_eq!(visible_packages(&drivers, "", false, false, false, 16, true), vec![1, 0, 2]);
        assert_eq!(visible_packages(&drivers, "", false, false, false, 8, false)[0], 1);
        assert_eq!(visible_packages(&drivers, "", false, false, false, 9, true)[0], 0);
        // Searching: signer, extension ID, install date and a file name that is shown.
        assert_eq!(visible_packages(&drivers, "acme", false, false, false, 0, false), vec![1]);
        assert_eq!(visible_packages(&drivers, "abcdef12", false, false, false, 0, false), vec![0]);
        assert_eq!(visible_packages(&drivers, "2024-03-05", false, false, false, 0, false), vec![0]);
        assert_eq!(visible_packages(&drivers, "a.cat", false, false, false, 0, false), vec![0]);
    }

    #[test]
    fn files_column_text() {
        let names = |n: usize| (1..=n).map(|i| format!("f{i}")).collect::<Vec<_>>();
        assert_eq!(format_files(&[]), "-");
        assert_eq!(format_files(&names(1)), "1 file: f1");
        assert_eq!(format_files(&names(5)), "5 files: f1, f2, f3, f4, f5");
        assert_eq!(format_files(&names(6)), "6 files: f1, f2, f3, f4, f5 and 1 more file");
        assert_eq!(format_files(&names(12)), "12 files: f1, f2, f3, f4, f5 and 7 more files");
    }

    #[test]
    fn partial_sizes_are_marked() {
        assert_eq!(format_size_text(5242880, true), "5.0 MB");
        assert_eq!(format_size_text(5242880, false), "5.0 MB+");
    }

    #[test]
    fn view_filter_sort_group() {
        let mut packages = vec![
            raw("oem10.inf", "a.inf", "1.0.0.0", (2020, 1, 1)),
            raw("oem2.inf", "a.inf", "2.0.0.0", (2021, 1, 1)),
            raw("oem3.inf", "z.inf", "1.0.0.0", (2020, 1, 1)),
        ];
        packages[2].class = "Display".into();
        let drivers = new_driver_records(&packages, &HashMap::new());

        // Default sort: by number (numeric, so oem2 < oem3 < oem10).
        let v = visible_packages(&drivers, "", false, false, false, 0, false);
        assert_eq!(v, vec![1, 2, 0]);
        let v = visible_packages(&drivers, "", false, false, false, 0, true);
        assert_eq!(v, vec![0, 2, 1]);
        // Filter (case-insensitive, trimmed) and old only.
        assert_eq!(visible_packages(&drivers, "  DISPLAY ", false, false, false, 0, false), vec![2]);
        assert_eq!(visible_packages(&drivers, "", true, false, false, 0, false), vec![0]);
        // Sort by original INF, ties broken by number.
        assert_eq!(visible_packages(&drivers, "", false, false, false, 1, false), vec![1, 0, 2]);

        let visible = visible_packages(&drivers, "", false, false, false, 0, false);
        let model = build_view(&drivers, &visible, Some(GroupBy::Class));
        assert_eq!(model.group_headers.len(), 2);
        assert_eq!(model.group_headers[0], "Display  -  1 package(s), 0 old, 1,000 B");
        assert_eq!(model.group_headers[1], "Net  -  2 package(s), 1 old, 2 KB");
        let order: Vec<usize> = model.rows.iter().map(|r| r.driver).collect();
        assert_eq!(order, vec![2, 1, 0]);

        let flat = build_view(&drivers, &visible, None);
        assert!(flat.group_headers.is_empty());
        assert!(flat.rows.iter().all(|r| r.group.is_none()));
    }

    #[test]
    fn summary() {
        let packages = vec![raw("oem1.inf", "a.inf", "1.0.0.0", (2020, 1, 1)), raw("oem2.inf", "a.inf", "2.0.0.0", (2021, 1, 1))];
        let mut drivers = new_driver_records(&packages, &HashMap::new());
        drivers[0].checked = true;
        assert_eq!(
            summary_text(&drivers, 2),
            "2 packages | 1 old (1 unused, 0 in use; 0 boot-critical) | 0 to review | 2 shown | 1 checked (1,000 B) | Light blue = old, unused | Light yellow = old, in use | Gray = review"
        );
    }

    #[test]
    fn package_number() {
        assert_eq!(parse_package_number("oem16.inf"), Ok(16));
        assert_eq!(parse_package_number("abc"), Ok(0));
    }
}
