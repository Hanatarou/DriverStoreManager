//! Devices, system information and the changes DISM makes to an OFFLINE image, WITHOUT PowerShell.
//!
//!  * Devices: Configuration Manager (CfgMgr32), including devices that are not plugged in right now.
//!    The device list gives which oemNN.inf each device uses, also through "extension" INFs.
//!  * System information: build number from `RtlGetVersion` (not affected by manifests) and the product name.
//!  * Adding and removing driver packages of an OFFLINE image: the DISM API (`dismapi.dll`).
//!
//! The driver packages themselves are read by `drvstore.rs`; this file keeps what it returns (`WindowsDriver`)
//! and the checks on it. Everything on the running Windows is changed through `pnputil.exe` (see pnputil.rs).

use crate::date::{Date, DateTime};
use crate::model::Signature;
use crate::netversion::NetVersion;

/// One entry of the Driver Store (the properties the program uses).
#[derive(Clone, Debug)]
pub struct WindowsDriver {
    /// "oem16.inf"
    pub driver: String,
    /// Full path of the INF inside the Driver Store.
    pub original_file_name: String,
    /// "net.inf"
    pub original_inf: String,
    /// Folder of the package inside the Driver Store.
    pub folder: String,
    pub provider: String,
    pub class: String,
    pub version: NetVersion,
    pub date: Date,
    pub boot_critical: bool,
    pub signature: Signature,
    /// "{guid}" in lower case; empty for a package that is not an extension.
    pub extension_id: String,
    pub signer: String,
    /// When the package was added to the Driver Store, in local time.
    pub install_date: Option<DateTime>,
}

/// What DISM reports about one .inf file, installed or not (a file in a folder).
#[derive(Clone, Debug)]
pub struct InfInfo {
    pub class: String,
    pub provider: String,
    pub version: NetVersion,
    pub date: Date,
}

/// Which Windows is being managed: the one that is running, or an offline image (the root folder that
/// contains the Windows folder, for example "D:\\").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageTarget {
    Online,
    Offline(String),
}

impl ImageTarget {
    pub fn is_offline(&self) -> bool {
        matches!(self, ImageTarget::Offline(_))
    }

    pub fn describe(&self) -> String {
        match self {
            ImageTarget::Online => "the running Windows".to_string(),
            ImageTarget::Offline(path) => format!("the offline image {path}"),
        }
    }
}

pub struct SystemInfo {
    pub caption: String,
    pub build_number: i64,
}

// ---- Pure helpers (tested on any platform) ----------------------------------------------------------

/// Split-Path -Leaf / -Parent for a Windows path: (leaf, parent).
pub fn split_path(path: &str) -> (String, String) {
    match path.rfind(['\\', '/']) {
        Some(i) => (path[i + 1..].to_string(), path[..i].to_string()),
        None => (path.to_string(), String::new()),
    }
}

/// A REG_MULTI_SZ / DEVPROP_TYPE_STRING_LIST buffer (UTF-16LE, NUL separated) as a list of strings.
pub fn parse_multi_sz(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    units
        .split(|&u| u == 0)
        .filter(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// A single UTF-16LE string buffer, up to the first NUL.
pub fn parse_sz(bytes: &[u8]) -> String {
    parse_multi_sz(bytes).into_iter().next().unwrap_or_default()
}

/// Windows keeps "Windows 10" in the ProductName registry value even on Windows 11 (build 22000 and later).
pub fn display_product_name(product: &str, build: i64) -> String {
    match product.strip_prefix("Windows 10") {
        Some(rest) if build >= 22000 => format!("Windows 11{rest}"),
        _ => product.to_string(),
    }
}

/// A path that the Driver Store reports for an OFFLINE image can still name the drive the image had when it was running
/// ("C:\\Windows\\System32\\..."). The same path below the image root ("D:\\Windows\\System32\\...") is returned.
pub fn rebase_to_image(path: &str, image_root: &str) -> Option<String> {
    let at = path.to_ascii_lowercase().find("\\windows\\")?;
    Some(format!("{}\\{}", image_root.trim_end_matches('\\'), &path[at + 1..]))
}

/// "oemNN.inf" (case-insensitive), the only names the Driver Store may report for third-party packages.
pub fn looks_like_published_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match lower.strip_prefix("oem").and_then(|rest| rest.strip_suffix(".inf")) {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// Plausibility check of one package read from the Driver Store: only what a wrong structure layout would break
/// (the names and the path). The DATE is deliberately not checked: INFs from some vendors carry very old or
/// empty dates (a package with such a date is perfectly valid), and Get-WindowsDriver accepted them too.
pub fn validate_driver(driver: &WindowsDriver) -> Result<(), String> {
    let lower_file = driver.original_file_name.to_ascii_lowercase();
    let reason = if !looks_like_published_name(&driver.driver) {
        Some("the published name is not oemNN.inf")
    } else if !lower_file.ends_with(".inf") {
        Some("the original file name does not end in .inf")
    } else if driver.original_inf.is_empty() {
        Some("the INF file name is empty")
    } else if driver.folder.is_empty() {
        Some("the package folder is empty")
    } else {
        None
    };
    match reason {
        None => Ok(()),
        Some(reason) => Err(format!(
            "The Driver Store returned data that does not look like a driver package ({reason}): '{}', '{}'. Nothing was loaded.",
            driver.driver, driver.original_file_name
        )),
    }
}

/// The date of a package with no usable date (all zero, or impossible): the earliest day, so that it never
/// counts as newer than another package.
pub const UNKNOWN_DATE: Date = Date { year: 1, month: 1, day: 1 };

/// Reads a NUL-terminated UTF-16 string owned by the Driver Store. A null pointer is an empty string.
///
/// # Safety
/// `pointer` must be null or point to a NUL-terminated UTF-16 string.
#[cfg_attr(not(windows), allow(dead_code))]
pub unsafe fn read_wide(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0usize;
    // A path or a name is never this long: the limit only protects against runaway reads.
    while length < 32768 && *pointer.add(length) != 0 {
        length += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length))
}

#[cfg(windows)]
pub use imp::{add_drivers_offline, get_devices, get_inf_info, get_system_info, remove_driver_offline};

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use anyhow::{anyhow, bail, Result};
    use windows::core::{w, PCSTR, PCWSTR, HRESULT};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
        CM_Locate_DevNodeW, CM_DEVNODE_STATUS_FLAGS, CM_PROB, CM_GETIDLIST_FILTER_NONE, CM_LOCATE_DEVNODE_PHANTOM,
        CR_BUFFER_SMALL, CR_SUCCESS,
    };
    use windows::Win32::Devices::Properties::{
        DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_DriverInfPath, DEVPKEY_Device_FriendlyName, DEVPROPTYPE,
    };
    use windows::Win32::Foundation::{DEVPROPKEY, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        GetModuleHandleW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    use super::*;
    use crate::model::DeviceRecord;
    use crate::proc;

    // ---- DISM ------------------------------------------------------------------------------------------

    /// DISM_ONLINE_IMAGE: "the Windows that is running now".
    const DISM_ONLINE_IMAGE: PCWSTR = w!("DISM_{53BFAE52-B167-4E2F-A258-0A37B57FF845}");
    /// DismLogErrors
    const DISM_LOG_ERRORS: i32 = 0;

    type DismInitializeFn = unsafe extern "system" fn(i32, PCWSTR, PCWSTR) -> i32;
    type DismOpenSessionFn = unsafe extern "system" fn(PCWSTR, PCWSTR, PCWSTR, *mut u32) -> i32;
    type DismAddDriverFn = unsafe extern "system" fn(u32, PCWSTR, i32) -> i32;
    type DismRemoveDriverFn = unsafe extern "system" fn(u32, PCWSTR) -> i32;
    type DismCloseSessionFn = unsafe extern "system" fn(u32) -> i32;
    type DismShutdownFn = unsafe extern "system" fn() -> i32;
    /// DismGetDriverInfo(Session, DriverPath, DismDriver** Driver, UINT* Count, DismDriverPackage** DriverPackage)
    type DismGetDriverInfoFn = unsafe extern "system" fn(u32, PCWSTR, *mut *mut c_void, *mut u32, *mut *mut c_void) -> i32;
    /// DismDelete(void* DismStructure): frees what the DISM API returned.
    type DismDeleteFn = unsafe extern "system" fn(*mut c_void) -> i32;

    struct Dism {
        initialize: DismInitializeFn,
        open_session: DismOpenSessionFn,
        add_driver: DismAddDriverFn,
        remove_driver: DismRemoveDriverFn,
        close_session: DismCloseSessionFn,
        shutdown: DismShutdownFn,
        get_driver_info: DismGetDriverInfoFn,
        delete: DismDeleteFn,
    }

    static DISM: OnceLock<std::result::Result<Dism, String>> = OnceLock::new();

    fn load_dism() -> std::result::Result<Dism, String> {
        unsafe {
            // System32 only: this program runs elevated and must never load a look-alike DLL from elsewhere.
            let module: HMODULE = LoadLibraryExW(w!("dismapi.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .map_err(|e| format!("dismapi.dll (the DISM API) could not be loaded from the System32 folder: {e}"))?;
            macro_rules! function {
                ($name:literal, $ty:ty) => {{
                    match GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr())) {
                        Some(address) => std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(address),
                        None => return Err(format!("dismapi.dll does not contain the function {}.", $name)),
                    }
                }};
            }
            Ok(Dism {
                initialize: function!("DismInitialize", DismInitializeFn),
                open_session: function!("DismOpenSession", DismOpenSessionFn),
                add_driver: function!("DismAddDriver", DismAddDriverFn),
                remove_driver: function!("DismRemoveDriver", DismRemoveDriverFn),
                close_session: function!("DismCloseSession", DismCloseSessionFn),
                shutdown: function!("DismShutdown", DismShutdownFn),
                get_driver_info: function!("DismGetDriverInfo", DismGetDriverInfoFn),
                delete: function!("DismDelete", DismDeleteFn),
            })
        }
    }

    fn dism() -> Result<&'static Dism> {
        DISM.get_or_init(load_dism).as_ref().map_err(|e| anyhow!("{e}"))
    }

    fn hresult_text(code: i32) -> String {
        let message = HRESULT(code).message();
        if message.trim().is_empty() {
            format!("error 0x{:08X}", code as u32)
        } else {
            format!("{} (HRESULT 0x{:08X})", message.trim(), code as u32)
        }
    }

    /// Runs `body` inside a DISM session on the running Windows or on an offline image, and always closes
    /// the session and shuts DISM down afterwards.
    fn with_session<T>(target: &ImageTarget, body: impl FnOnce(&Dism, u32) -> Result<T>) -> Result<T> {
        let dism = dism()?;
        unsafe {
            let hr = (dism.initialize)(DISM_LOG_ERRORS, PCWSTR::null(), PCWSTR::null());
            if hr < 0 {
                bail!("The DISM API could not be started: {}", hresult_text(hr));
            }
            // From here on every path must call DismShutdown.
            let result = (|| -> Result<T> {
                let image_path = match target {
                    ImageTarget::Online => None,
                    ImageTarget::Offline(path) => Some(crate::win::wide(path)),
                };
                let image = match &image_path {
                    None => DISM_ONLINE_IMAGE,
                    Some(wide) => PCWSTR(wide.as_ptr()),
                };
                let mut session = 0u32;
                let hr = (dism.open_session)(image, PCWSTR::null(), PCWSTR::null(), &mut session);
                if hr < 0 {
                    bail!("DISM could not open {}: {}", target.describe(), hresult_text(hr));
                }
                let outcome = body(dism, session);
                (dism.close_session)(session);
                outcome
            })();
            (dism.shutdown)();
            result
        }
    }

    /// Adds driver packages (given by the path of their .inf file) to an OFFLINE image. One result per file.
    pub fn add_drivers_offline(target: &ImageTarget, infs: &[std::path::PathBuf]) -> Result<Vec<(std::path::PathBuf, std::result::Result<(), String>)>> {
        with_session(target, |dism, session| {
            let mut results = Vec::with_capacity(infs.len());
            for inf in infs {
                let wide = crate::win::wide(&inf.to_string_lossy());
                // ForceUnsigned = FALSE: an unsigned driver is refused, as it is everywhere else in this program.
                let hr = unsafe { (dism.add_driver)(session, PCWSTR(wide.as_ptr()), 0) };
                results.push((inf.clone(), if hr < 0 { Err(hresult_text(hr)) } else { Ok(()) }));
                crate::proc::pump_throttled();
            }
            Ok(results)
        })
    }

    /// DismDriverPackage (dismapi.h). The header packs the structures, so every field sits right after the
    /// previous one at 4-byte steps (a pointer is not padded to 8 bytes).
    #[repr(C, packed(4))]
    #[derive(Clone, Copy)]
    struct DismDriverPackage {
        published_name: *const u16,
        original_file_name: *const u16,
        in_box: i32,
        catalog_file: *const u16,
        class_name: *const u16,
        class_guid: *const u16,
        class_description: *const u16,
        boot_critical: i32,
        driver_signature: i32,
        provider_name: *const u16,
        /// SYSTEMTIME: year, month, day of week, day, hour, minute, second, milliseconds.
        date: [u16; 8],
        major: u32,
        minor: u32,
        build: u32,
        revision: u32,
    }

    /// Text of a NUL-terminated UTF-16 string returned by DISM (empty for a null pointer).
    unsafe fn dism_text(text: *const u16) -> String {
        if text.is_null() {
            return String::new();
        }
        let mut length = 0usize;
        while length < 4096 && *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, length)).trim().to_string()
    }

    /// Reads the class, provider, version and date of the package DISM returned. The date and the version are
    /// checked BEFORE any text is read: if the layout of the structure were not what is expected here, they
    /// would not look like a date and a version, and nothing is read through the other pointers.
    unsafe fn read_inf_info(package: *const DismDriverPackage) -> std::result::Result<InfInfo, String> {
        let info = std::ptr::read_unaligned(package);
        let (date, major, minor, build, revision) = (info.date, info.major, info.minor, info.build, info.revision);
        let (year, month, day) = (date[0] as i32, date[1] as u32, date[3] as u32);
        let plausible_date = (1980..=2200).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day);
        let plausible_version = [major, minor, build, revision].iter().all(|&v| v <= i32::MAX as u32);
        if !plausible_date || !plausible_version {
            return Err("DISM returned data that does not look like a driver date and version, so it was not used.".to_string());
        }
        Ok(InfInfo {
            class: dism_text(info.class_name),
            provider: dism_text(info.provider_name),
            version: NetVersion { major: major as i32, minor: minor as i32, build: build as i32, revision: revision as i32 },
            date: Date { year, month, day },
        })
    }

    /// Reads, with DISM, the class, provider, version and date of each .inf file. The files do not have to be
    /// installed. One result per file; a file DISM cannot read gives an error text, not an error for the call.
    pub fn get_inf_info(
        target: &ImageTarget,
        infs: &[std::path::PathBuf],
    ) -> Result<Vec<(std::path::PathBuf, std::result::Result<InfInfo, String>)>> {
        with_session(target, |dism, session| {
            let mut results = Vec::with_capacity(infs.len());
            for inf in infs {
                let wide = crate::win::wide(&inf.to_string_lossy());
                let mut drivers: *mut c_void = std::ptr::null_mut();
                let mut count = 0u32;
                let mut package: *mut c_void = std::ptr::null_mut();
                let hr = unsafe { (dism.get_driver_info)(session, PCWSTR(wide.as_ptr()), &mut drivers, &mut count, &mut package) };
                let outcome = if hr < 0 {
                    Err(hresult_text(hr))
                } else if package.is_null() {
                    Err("DISM returned no package information.".to_string())
                } else {
                    unsafe { read_inf_info(package as *const DismDriverPackage) }
                };
                unsafe {
                    if !drivers.is_null() {
                        (dism.delete)(drivers);
                    }
                    if !package.is_null() {
                        (dism.delete)(package);
                    }
                }
                results.push((inf.clone(), outcome));
                crate::proc::pump_throttled();
            }
            Ok(results)
        })
    }

    /// Removes a driver package, by its published name ("oem5.inf"), from an OFFLINE image.
    pub fn remove_driver_offline(target: &ImageTarget, published_name: &str) -> Result<()> {
        with_session(target, |dism, session| {
            let wide = crate::win::wide(published_name);
            let hr = unsafe { (dism.remove_driver)(session, PCWSTR(wide.as_ptr())) };
            if hr < 0 {
                bail!("{}", hresult_text(hr));
            }
            Ok(())
        })
    }

    // ---- Devices (Configuration Manager) -----------------------------------------------------------------

    /// DEVPKEY_Device_DriverExtendedInfs: the extension INFs (oemNN.inf) that also apply to a device.
    const DEVPKEY_DEVICE_DRIVER_EXTENDED_INFS: DEVPROPKEY = DEVPROPKEY {
        fmtid: windows::core::GUID::from_u128(0xa8b865dd_2e3d_4094_ad97_e593a70c75d6),
        pid: 20,
    };

    fn device_property(devinst: u32, key: &DEVPROPKEY) -> Option<Vec<u8>> {
        unsafe {
            let mut property_type = DEVPROPTYPE(0);
            let mut size = 0u32;
            let r = CM_Get_DevNode_PropertyW(devinst, key, &mut property_type, None, &mut size, 0);
            if (r != CR_BUFFER_SMALL && r != CR_SUCCESS) || size == 0 {
                return None;
            }
            let mut buffer = vec![0u8; size as usize];
            let r = CM_Get_DevNode_PropertyW(devinst, key, &mut property_type, Some(buffer.as_mut_ptr()), &mut size, 0);
            if r != CR_SUCCESS {
                return None;
            }
            buffer.truncate(size as usize);
            Some(buffer)
        }
    }

    /// Instance IDs of every device Windows knows, present or not.
    fn device_ids() -> Result<Vec<String>> {
        unsafe {
            for _ in 0..4 {
                let mut length = 0u32;
                let r = CM_Get_Device_ID_List_SizeW(&mut length, PCWSTR::null(), CM_GETIDLIST_FILTER_NONE);
                if r != CR_SUCCESS {
                    bail!("The device list could not be sized (Configuration Manager error {}).", r.0);
                }
                let mut buffer = vec![0u16; length as usize + 2];
                let r = CM_Get_Device_ID_ListW(PCWSTR::null(), &mut buffer, CM_GETIDLIST_FILTER_NONE);
                if r == CR_BUFFER_SMALL {
                    // The list grew between the two calls (a device was plugged in): try again.
                    continue;
                }
                if r != CR_SUCCESS {
                    bail!("The device list could not be read (Configuration Manager error {}).", r.0);
                }
                return Ok(buffer
                    .split(|&u| u == 0)
                    .filter(|part| !part.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect());
            }
            bail!("The device list kept changing while it was read. Try again.")
        }
    }

    /// Every device with the driver package it uses (Get-PnpDevice + Win32_PnPSignedDriver).
    pub fn get_devices() -> Result<Vec<DeviceRecord>> {
        let ids = device_ids()?;
        let mut devices = Vec::with_capacity(ids.len());
        for instance_id in ids {
            let mut devinst = 0u32;
            let wide: Vec<u16> = instance_id.encode_utf16().chain(std::iter::once(0)).collect();
            if unsafe { CM_Locate_DevNodeW(&mut devinst, PCWSTR(wide.as_ptr()), CM_LOCATE_DEVNODE_PHANTOM) } != CR_SUCCESS {
                continue; // the device disappeared while the list was read
            }
            let inf = device_property(devinst, &DEVPKEY_Device_DriverInfPath).map(|b| parse_sz(&b)).unwrap_or_default();
            let extended_infs = device_property(devinst, &DEVPKEY_DEVICE_DRIVER_EXTENDED_INFS)
                .map(|b| parse_multi_sz(&b))
                .unwrap_or_default();
            if inf.is_empty() && extended_infs.is_empty() {
                continue; // no driver package: nothing to map
            }
            let name = device_property(devinst, &DEVPKEY_Device_FriendlyName)
                .map(|b| parse_sz(&b))
                .filter(|n| !n.is_empty())
                .or_else(|| device_property(devinst, &DEVPKEY_Device_DeviceDesc).map(|b| parse_sz(&b)))
                .unwrap_or_default();
            // A device that is not plugged in has no live device node: the status call says so. For one that
            // is plugged in, the same call gives its Device Manager problem code (0 = no problem).
            let (present, problem) = unsafe {
                let mut status = CM_DEVNODE_STATUS_FLAGS(0);
                let mut problem = CM_PROB(0);
                let present = CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) == CR_SUCCESS;
                (present, if present { problem.0 } else { 0 })
            };
            devices.push(DeviceRecord { instance_id, name, inf, extended_infs, present, problem });
            proc::pump_throttled();
        }
        Ok(devices)
    }

    // ---- System information --------------------------------------------------------------------------------

    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }

    /// RtlGetVersion tells the real build even for programs without a Windows 10 manifest.
    fn build_number() -> Result<i64> {
        unsafe {
            let ntdll = GetModuleHandleW(w!("ntdll.dll")).map_err(|e| anyhow!("ntdll.dll was not found: {e}"))?;
            let address = GetProcAddress(ntdll, PCSTR(b"RtlGetVersion\0".as_ptr()))
                .ok_or_else(|| anyhow!("Windows did not return the operating system information."))?;
            let function = std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(*mut OsVersionInfo) -> i32>(address);
            let mut info = OsVersionInfo { size: std::mem::size_of::<OsVersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
            if function(&mut info) != 0 {
                bail!("Windows did not return the operating system information.");
            }
            Ok(info.build as i64)
        }
    }

    fn registry_text(name: PCWSTR) -> Option<String> {
        unsafe {
            let subkey = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
            let mut buffer = [0u16; 256];
            let mut size = (buffer.len() * 2) as u32;
            let status = RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey,
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr() as *mut c_void),
                Some(&mut size),
            );
            if status.0 != 0 {
                return None;
            }
            let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..end]))
        }
    }

    pub fn get_system_info() -> Result<SystemInfo> {
        let build_number = build_number()?;
        let product = display_product_name(&registry_text(w!("ProductName")).unwrap_or_else(|| "Windows".to_string()), build_number);
        let version = registry_text(w!("DisplayVersion")).map(|v| format!(" {v}")).unwrap_or_default();
        Ok(SystemInfo { caption: format!("{product}{version}"), build_number })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn offline_paths_are_rebased_below_the_image_root() {
        assert_eq!(
            rebase_to_image("C:\\Windows\\System32\\DriverStore\\FileRepository\\a.inf_x\\a.inf", "D:\\").as_deref(),
            Some("D:\\Windows\\System32\\DriverStore\\FileRepository\\a.inf_x\\a.inf")
        );
        assert_eq!(rebase_to_image("c:\\WINDOWS\\x\\a.inf", "E:\\img").as_deref(), Some("E:\\img\\WINDOWS\\x\\a.inf"));
        assert_eq!(rebase_to_image("C:\\Other\\a.inf", "D:\\"), None);
        assert!(ImageTarget::Offline("D:\\".into()).is_offline() && !ImageTarget::Online.is_offline());
    }

    #[test]
    fn windows_11_is_named_by_build() {
        assert_eq!(display_product_name("Windows 10 Pro", 26300), "Windows 11 Pro");
        assert_eq!(display_product_name("Windows 10 Pro", 19045), "Windows 10 Pro");
        assert_eq!(display_product_name("Windows 11 Pro", 26100), "Windows 11 Pro");
        assert_eq!(display_product_name("Windows Server 2022", 20348), "Windows Server 2022");
    }

    #[test]
    fn splits_paths_like_split_path() {
        assert_eq!(
            split_path("C:\\Windows\\System32\\DriverStore\\FileRepository\\net.inf_amd64_1\\net.inf"),
            ("net.inf".to_string(), "C:\\Windows\\System32\\DriverStore\\FileRepository\\net.inf_amd64_1".to_string())
        );
        assert_eq!(split_path("net.inf"), ("net.inf".to_string(), String::new()));
        assert_eq!(split_path("/fake/a/b.inf"), ("b.inf".to_string(), "/fake/a".to_string()));
    }

    #[test]
    fn reads_device_property_strings() {
        let mut one = utf16("oem12.inf");
        one.extend_from_slice(&[0, 0]);
        assert_eq!(parse_sz(&one), "oem12.inf");
        assert_eq!(parse_sz(&[]), "");

        let mut list = utf16("oem3.inf");
        list.extend_from_slice(&[0, 0]);
        list.extend(utf16("oem4.inf"));
        list.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(parse_multi_sz(&list), vec!["oem3.inf".to_string(), "oem4.inf".to_string()]);
        assert!(parse_multi_sz(&[0, 0, 0, 0]).is_empty());
    }

    #[test]
    fn published_names_and_validation() {
        assert!(looks_like_published_name("oem16.inf"));
        assert!(looks_like_published_name("OEM1.INF"));
        assert!(!looks_like_published_name("oem.inf"));
        assert!(!looks_like_published_name("net.inf"));
        assert!(!looks_like_published_name("oem1x.inf"));

        let mut driver = WindowsDriver {
            driver: "oem7.inf".into(),
            original_file_name: "C:\\Windows\\System32\\DriverStore\\FileRepository\\net.inf_x\\net.inf".into(),
            original_inf: "net.inf".into(),
            folder: "C:\\Windows\\System32\\DriverStore\\FileRepository\\net.inf_x".into(),
            provider: "Intel".into(),
            class: "Net".into(),
            version: NetVersion::parse("1.2.3.4").unwrap(),
            date: Date::from_ymd_opt(2024, 3, 5).unwrap(),
            boot_critical: false,
            signature: Signature(0x0D00_0005),
            extension_id: String::new(),
            signer: String::new(),
            install_date: None,
        };
        assert!(validate_driver(&driver).is_ok());
        driver.date = UNKNOWN_DATE;
        assert!(validate_driver(&driver).is_ok()); // the date never makes a package invalid
        driver.driver = "garbage".into();
        assert!(validate_driver(&driver).is_err());
    }
}
