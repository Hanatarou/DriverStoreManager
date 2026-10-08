//! Reads the Driver Store of the running Windows or of an offline image through `drvstore.dll`, the Driver
//! Store library that `pnputil.exe`, `setupapi.dll` and `drvinst.exe` use. It is not documented, so the rules
//! here are strict:
//!
//!  * The DLL is loaded from the System32 folder only and every function is looked up BY NAME (the ordinals
//!    change between Windows builds). A missing DLL or function is a clear error, never a guess.
//!  * Every value is checked against the type Windows says it has and the size that type needs. A value
//!    that does not fit is treated as absent, never reinterpreted.
//!  * The class GUID of each package is read twice, once as a property and once from the package's version
//!    information. If they differ, the structure layout is not what this code expects and reading stops
//!    with an error instead of showing (or removing) wrong packages.
//!
//! The functions used (all `...W`): DriverStoreOpen/Close/Enum/GetObjectProperty and
//! DriverPackageOpen/GetVersionInfo/Close. The property keys are {8163eb01-142c-4f7a-94e1-a274cc47dbba}
//! with the ids below (the same names as the values under HKLM\SYSTEM\DriverDatabase).
//!
//! Nothing here changes the system.

use crate::native::parse_sz;
use crate::netversion::NetVersion;

// ---- Property values (pure, tested on any platform) -----------------------------------------------------

/// DEVPROPTYPE values (devpropdef.h) of the properties that are read.
const TYPE_UINT16: u32 = 0x05;
const TYPE_UINT32: u32 = 0x07;
const TYPE_UINT64: u32 = 0x09;
const TYPE_GUID: u32 = 0x0D;
const TYPE_FILETIME: u32 = 0x10;
const TYPE_BOOLEAN: u32 = 0x11;
const TYPE_STRING: u32 = 0x12;

/// A GUID with the layout of the Windows structure (a property of type GUID is these 16 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    fn from_bytes(bytes: &[u8]) -> Option<Guid> {
        if bytes.len() != 16 {
            return None;
        }
        Some(Guid {
            data1: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            data2: u16::from_le_bytes(bytes[4..6].try_into().ok()?),
            data3: u16::from_le_bytes(bytes[6..8].try_into().ok()?),
            data4: bytes[8..16].try_into().ok()?,
        })
    }

    fn is_nil(&self) -> bool {
        *self == Guid::default()
    }

    /// "{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}" in lower case.
    fn braced(&self) -> String {
        let d = self.data4;
        format!(
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

fn decode_string(kind: u32, bytes: &[u8]) -> Option<String> {
    (kind == TYPE_STRING).then(|| parse_sz(bytes))
}

fn decode_u16(kind: u32, bytes: &[u8]) -> Option<u16> {
    (kind == TYPE_UINT16).then(|| bytes.try_into().ok().map(u16::from_le_bytes)).flatten()
}

fn decode_u32(kind: u32, bytes: &[u8]) -> Option<u32> {
    (kind == TYPE_UINT32).then(|| bytes.try_into().ok().map(u32::from_le_bytes)).flatten()
}

fn decode_u64(kind: u32, bytes: &[u8]) -> Option<u64> {
    (kind == TYPE_UINT64).then(|| bytes.try_into().ok().map(u64::from_le_bytes)).flatten()
}

fn decode_guid(kind: u32, bytes: &[u8]) -> Option<Guid> {
    (kind == TYPE_GUID).then(|| Guid::from_bytes(bytes)).flatten()
}

/// A FILETIME property: the 100-nanosecond ticks since 1601 (UTC).
fn decode_filetime(kind: u32, bytes: &[u8]) -> Option<u64> {
    (kind == TYPE_FILETIME).then(|| bytes.try_into().ok().map(u64::from_le_bytes)).flatten()
}

/// A boolean property is one byte; any value but 0 is true.
fn decode_bool(kind: u32, bytes: &[u8]) -> Option<bool> {
    (kind == TYPE_BOOLEAN && bytes.len() == 1).then(|| bytes[0] != 0)
}

/// The driver version as Windows stores it: four 16-bit numbers, the major one in the highest bits.
fn unpack_version(value: u64) -> NetVersion {
    NetVersion {
        major: ((value >> 48) & 0xFFFF) as i32,
        minor: ((value >> 32) & 0xFFFF) as i32,
        build: ((value >> 16) & 0xFFFF) as i32,
        revision: (value & 0xFFFF) as i32,
    }
}

/// The extension ID as "{guid}" in lower case; a package that is not an extension has none (empty).
fn extension_id_text(guid: Option<Guid>) -> String {
    match guid {
        Some(guid) if !guid.is_nil() => guid.braced(),
        _ => String::new(),
    }
}

// ---- Structures (layout of the Windows structures; tested below) -----------------------------------------

const LOCALE_NAME_MAX_LENGTH: usize = 85;
const MAX_PATH: usize = 260;

/// `DriverPackageInfo`: what the enumeration gives for each package (only the published name is used).
#[repr(C)]
#[cfg_attr(not(windows), allow(dead_code))]
struct DriverPackageInfo {
    architecture: u16,
    locale_name: [u16; LOCALE_NAME_MAX_LENGTH],
    published_inf_name: [u16; MAX_PATH],
    flags: u32,
}

/// `DriverPackageVersionInfo`: filled by DriverPackageGetVersionInfo. Only `size` is set before the call.
#[repr(C)]
#[cfg_attr(not(windows), allow(dead_code))]
struct DriverPackageVersionInfo {
    size: u32,
    architecture: u16,
    locale_name: [u16; LOCALE_NAME_MAX_LENGTH],
    provider_name: [u16; MAX_PATH],
    date_low: u32,
    date_high: u32,
    version: u64,
    class_guid: Guid,
    class_name: [u16; MAX_PATH],
    class_version: u32,
    catalog_file: [u16; MAX_PATH],
    flags: u32,
}

/// DriverPackageVersionInfo.Flags: the INF forces the package to be (or not to be) boot-critical.
const FORCE_BOOT_CRITICAL: u32 = 0x4;
const FORCE_NOT_BOOT_CRITICAL: u32 = 0x8;

/// A NUL-terminated UTF-16 text inside a fixed array.
#[cfg_attr(not(windows), allow(dead_code))]
fn array_text(array: &[u16]) -> String {
    let end = array.iter().position(|&c| c == 0).unwrap_or(array.len());
    String::from_utf16_lossy(&array[..end])
}

/// Boot-critical from the flags of the version information: Some(true / false) when the INF forces it.
#[cfg_attr(not(windows), allow(dead_code))]
fn forced_boot_critical(flags: u32) -> Option<bool> {
    if flags & FORCE_BOOT_CRITICAL != 0 {
        Some(true)
    } else if flags & FORCE_NOT_BOOT_CRITICAL != 0 {
        Some(false)
    } else {
        None
    }
}

#[cfg(windows)]
pub use imp::{get_windows_drivers, requirement_problem};

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use anyhow::{anyhow, bail, Result};
    use windows::core::{w, BOOL, PCSTR, PCWSTR};
    use windows::Win32::Foundation::{DEVPROPKEY, HMODULE};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    use super::*;
    use crate::date::DateTime;
    use crate::fsops::clean_io;
    use crate::model::Signature;
    use crate::native::{read_wide, split_path, validate_driver, ImageTarget, WindowsDriver, UNKNOWN_DATE};

    // DriverStoreObjectType
    const OBJECT_DRIVER_DATABASE: u32 = 1;
    const OBJECT_DRIVER_PACKAGE: u32 = 2;
    const OBJECT_DEVICE_SETUP_CLASS: u32 = 6;
    /// DriverStoreEnum: only OEM (third-party) packages, like `Get-WindowsDriver` without -All.
    const ENUM_OEM_ONLY: u32 = 0x2;
    /// DriverPackageOpen: open for the version information only.
    const OPEN_VERSION_ONLY: u32 = 0x1;
    /// Room for any value that is read (the longest are the provider and signer names).
    const PROPERTY_BUFFER: usize = 4096;

    /// Property keys {8163eb01-142c-4f7a-94e1-a274cc47dbba}, id N (DriverPackage).
    const fn package_key(id: u32) -> DEVPROPKEY {
        DEVPROPKEY { fmtid: windows::core::GUID::from_u128(0x8163eb01_142c_4f7a_94e1_a274cc47dbba), pid: id }
    }
    const KEY_SIGNER_NAME: DEVPROPKEY = package_key(7);
    const KEY_SIGNER_SCORE: DEVPROPKEY = package_key(8);
    const KEY_PROVIDER_NAME: DEVPROPKEY = package_key(12);
    const KEY_CLASS_GUID: DEVPROPKEY = package_key(13);
    const KEY_DRIVER_DATE: DEVPROPKEY = package_key(14);
    const KEY_DRIVER_VERSION: DEVPROPKEY = package_key(15);
    const KEY_EXTENSION_ID: DEVPROPKEY = package_key(20);
    const KEY_IMPORT_DATE: DEVPROPKEY = package_key(26);
    /// DriverDatabase, processor architecture of the store.
    const KEY_ARCHITECTURE: DEVPROPKEY = DEVPROPKEY {
        fmtid: windows::core::GUID::from_u128(0x8163eb00_142c_4f7a_94e1_a274cc47dbba),
        pid: 3,
    };
    /// DEVPKEY_DeviceClass_BootCritical, a property of the device setup class.
    const KEY_CLASS_BOOT_CRITICAL: DEVPROPKEY = DEVPROPKEY {
        fmtid: windows::core::GUID::from_u128(0x6a3433f4_5626_40e8_a9b9_dbd9ecd2884b),
        pid: 3,
    };

    type Handle = *mut c_void;
    type OpenFn = unsafe extern "system" fn(PCWSTR, PCWSTR, u32, Handle) -> Handle;
    type CloseFn = unsafe extern "system" fn(Handle) -> BOOL;
    type EnumCallback = unsafe extern "system" fn(Handle, PCWSTR, *const DriverPackageInfo, isize) -> BOOL;
    type EnumFn = unsafe extern "system" fn(Handle, u32, EnumCallback, isize) -> BOOL;
    type GetPropertyFn =
        unsafe extern "system" fn(Handle, u32, PCWSTR, *const DEVPROPKEY, *mut u32, *mut u8, i32, *mut u32, u32) -> BOOL;
    type PackageOpenFn = unsafe extern "system" fn(PCWSTR, u16, PCWSTR, u32, Handle) -> Handle;
    type VersionInfoFn = unsafe extern "system" fn(Handle, *mut DriverPackageVersionInfo) -> BOOL;
    type PackageCloseFn = unsafe extern "system" fn(Handle);

    struct Api {
        open: OpenFn,
        close: CloseFn,
        enumerate: EnumFn,
        get_property: GetPropertyFn,
        package_open: PackageOpenFn,
        version_info: VersionInfoFn,
        package_close: PackageCloseFn,
    }

    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();

    fn load() -> std::result::Result<Api, String> {
        unsafe {
            // System32 only: this program runs elevated and must never load a look-alike DLL from elsewhere.
            let module: HMODULE = LoadLibraryExW(w!("drvstore.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .map_err(|e| format!("drvstore.dll (the Driver Store library) could not be loaded from the System32 folder: {e}"))?;
            macro_rules! function {
                ($name:literal, $ty:ty) => {{
                    match GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr())) {
                        Some(address) => std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(address),
                        None => return Err(format!("drvstore.dll does not contain the function {}.", $name)),
                    }
                }};
            }
            Ok(Api {
                open: function!("DriverStoreOpenW", OpenFn),
                close: function!("DriverStoreClose", CloseFn),
                enumerate: function!("DriverStoreEnumW", EnumFn),
                get_property: function!("DriverStoreGetObjectPropertyW", GetPropertyFn),
                package_open: function!("DriverPackageOpenW", PackageOpenFn),
                version_info: function!("DriverPackageGetVersionInfoW", VersionInfoFn),
                package_close: function!("DriverPackageClose", PackageCloseFn),
            })
        }
    }

    fn api() -> Result<&'static Api> {
        API.get_or_init(load).as_ref().map_err(|e| anyhow!("{e}"))
    }

    /// Why the Driver Store cannot be read on this computer, or None when drvstore.dll is usable.
    pub fn requirement_problem() -> Option<String> {
        api().err().map(|e| format!("{e} It is part of Windows 10 and later."))
    }

    fn last_error() -> String {
        clean_io(&std::io::Error::last_os_error())
    }

    /// An open Driver Store, closed when dropped.
    struct Store {
        api: &'static Api,
        handle: Handle,
    }

    impl Drop for Store {
        fn drop(&mut self) {
            unsafe {
                let _ = (self.api.close)(self.handle);
            }
        }
    }

    impl Store {
        fn open(target: &ImageTarget) -> Result<Store> {
            let api = api()?;
            let handle = unsafe {
                match target {
                    ImageTarget::Online => (api.open)(PCWSTR::null(), PCWSTR::null(), 0, std::ptr::null_mut()),
                    ImageTarget::Offline(root) => {
                        let system = crate::win::wide(&std::path::Path::new(root).join("Windows").to_string_lossy());
                        let boot = crate::win::wide(root);
                        (api.open)(PCWSTR(system.as_ptr()), PCWSTR(boot.as_ptr()), 0, std::ptr::null_mut())
                    }
                }
            };
            if handle.is_null() {
                bail!("The Driver Store of {} could not be opened: {}", target.describe(), last_error());
            }
            Ok(Store { api, handle })
        }

        /// The type and bytes of one property of an object, or None when the object has no such property.
        fn property(&self, object_type: u32, name: PCWSTR, key: &DEVPROPKEY) -> Option<(u32, Vec<u8>)> {
            let mut buffer = vec![0u8; PROPERTY_BUFFER];
            let mut kind = 0u32;
            let mut size = 0u32;
            let found = unsafe {
                (self.api.get_property)(
                    self.handle,
                    object_type,
                    name,
                    key,
                    &mut kind,
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                    &mut size,
                    0,
                )
            };
            if !found.as_bool() || size == 0 || size as usize > buffer.len() {
                return None;
            }
            buffer.truncate(size as usize);
            Some((kind, buffer))
        }

        fn read<T>(&self, object_type: u32, name: PCWSTR, key: &DEVPROPKEY, decode: fn(u32, &[u8]) -> Option<T>) -> Option<T> {
            let (kind, bytes) = self.property(object_type, name, key)?;
            decode(kind, &bytes)
        }

        fn architecture(&self) -> Result<u16> {
            self.read(OBJECT_DRIVER_DATABASE, w!("SYSTEM"), &KEY_ARCHITECTURE, decode_u16)
                .ok_or_else(|| anyhow!("The Driver Store did not say which processor architecture it is for."))
        }

        /// Boot-critical: forced by the INF, otherwise a property of the device setup class, otherwise no.
        fn class_is_boot_critical(&self, class_guid: Guid) -> bool {
            let name = crate::win::wide(&class_guid.braced());
            self.read(OBJECT_DEVICE_SETUP_CLASS, PCWSTR(name.as_ptr()), &KEY_CLASS_BOOT_CRITICAL, decode_bool)
                .unwrap_or(false)
        }
    }

    /// The version information of a package: its class name, class GUID and whether the INF forces boot-critical.
    struct VersionInfo {
        class_name: String,
        class_guid: Guid,
        forced_boot_critical: Option<bool>,
    }

    fn version_info(store: &Store, inf_path: PCWSTR, architecture: u16) -> Result<VersionInfo> {
        unsafe {
            let package = (store.api.package_open)(inf_path, architecture, PCWSTR::null(), OPEN_VERSION_ONLY, std::ptr::null_mut());
            if package.is_null() {
                bail!("{}", last_error());
            }
            let mut info: DriverPackageVersionInfo = std::mem::zeroed();
            info.size = std::mem::size_of::<DriverPackageVersionInfo>() as u32;
            let read = (store.api.version_info)(package, &mut info);
            let error = last_error();
            (store.api.package_close)(package);
            if !read.as_bool() {
                bail!("{error}");
            }
            Ok(VersionInfo {
                class_name: array_text(&info.class_name),
                class_guid: info.class_guid,
                forced_boot_critical: forced_boot_critical(info.flags),
            })
        }
    }

    /// One package of the store, named by the full path of its INF.
    struct Package<'a> {
        store: &'a Store,
        inf_path: PCWSTR,
    }

    impl Package<'_> {
        fn get<T>(&self, key: &DEVPROPKEY, decode: fn(u32, &[u8]) -> Option<T>) -> Option<T> {
            self.store.read(OBJECT_DRIVER_PACKAGE, self.inf_path, key, decode)
        }
    }

    fn read_package(store: &Store, inf_path: PCWSTR, info: &DriverPackageInfo, architecture: u16) -> Result<WindowsDriver> {
        let published_name = array_text(&info.published_inf_name);
        let original_file_name = unsafe { read_wide(inf_path.0) };
        let (original_inf, folder) = split_path(&original_file_name);
        let package = Package { store, inf_path };

        let class_guid = package.get(&KEY_CLASS_GUID, decode_guid).unwrap_or_default();
        let details = version_info(store, inf_path, architecture)
            .map_err(|e| anyhow!("The Driver Store could not read the version information of {published_name}: {e}"))?;
        if details.class_guid != class_guid {
            bail!(
                "The Driver Store returned data that does not match its own description ({published_name} has two different class GUIDs). Nothing was loaded."
            );
        }

        let version = match package.get(&KEY_DRIVER_VERSION, decode_u64) {
            Some(value) => unpack_version(value),
            None => {
                crate::applog::warn(&format!("{published_name} ({original_inf}): the Driver Store gave no version. It is treated as 0.0.0.0."));
                NetVersion { major: 0, minor: 0, build: 0, revision: 0 }
            }
        };
        // An empty or impossible date becomes UNKNOWN_DATE.
        let date = match package.get(&KEY_DRIVER_DATE, decode_filetime).and_then(DateTime::from_filetime) {
            Some(value) => value.date,
            None => {
                crate::applog::warn(&format!(
                    "{published_name} ({original_inf}): the Driver Store gave no valid date. It is treated as 0001-01-01."
                ));
                UNKNOWN_DATE
            }
        };

        Ok(WindowsDriver {
            driver: published_name,
            original_file_name,
            original_inf,
            folder,
            provider: package.get(&KEY_PROVIDER_NAME, decode_string).unwrap_or_default(),
            class: details.class_name,
            version,
            date,
            boot_critical: details.forced_boot_critical.unwrap_or_else(|| store.class_is_boot_critical(class_guid)),
            signature: Signature(package.get(&KEY_SIGNER_SCORE, decode_u32).unwrap_or(0)),
            extension_id: extension_id_text(package.get(&KEY_EXTENSION_ID, decode_guid)),
            signer: package.get(&KEY_SIGNER_NAME, decode_string).unwrap_or_default(),
            install_date: package.get(&KEY_IMPORT_DATE, decode_filetime).and_then(DateTime::from_filetime),
        })
    }

    /// What the enumeration callback works with.
    struct Scan<'a> {
        store: &'a Store,
        architecture: u16,
        drivers: Vec<WindowsDriver>,
        error: Option<String>,
    }

    unsafe extern "system" fn on_package(_store: Handle, inf_path: PCWSTR, info: *const DriverPackageInfo, context: isize) -> BOOL {
        let scan = &mut *(context as *mut Scan);
        let outcome = read_package(scan.store, inf_path, &*info, scan.architecture)
            .and_then(|driver| validate_driver(&driver).map(|_| driver).map_err(|e| anyhow!("{e}")));
        crate::proc::pump_throttled();
        match outcome {
            Ok(driver) => {
                scan.drivers.push(driver);
                BOOL(1)
            }
            Err(error) => {
                scan.error = Some(error.to_string());
                BOOL(0) // stop the enumeration
            }
        }
    }

    /// Third-party driver packages of the running Windows (Get-WindowsDriver -Online) or of an offline image.
    pub fn get_windows_drivers(target: &ImageTarget) -> Result<Vec<WindowsDriver>> {
        let store = Store::open(target)?;
        let mut scan = Scan { store: &store, architecture: store.architecture()?, drivers: Vec::new(), error: None };
        let finished = unsafe { (store.api.enumerate)(store.handle, ENUM_OEM_ONLY, on_package, &mut scan as *mut Scan as isize) };
        if let Some(error) = scan.error {
            bail!("{error}");
        }
        if !finished.as_bool() {
            bail!("The Driver Store of {} could not be listed: {}", target.describe(), last_error());
        }
        Ok(scan.drivers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::{Date, DateTime};
    use std::mem::{offset_of, size_of};

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn structure_layouts_match_the_windows_structures() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(offset_of!(DriverPackageInfo, architecture), 0);
        assert_eq!(offset_of!(DriverPackageInfo, locale_name), 2);
        assert_eq!(offset_of!(DriverPackageInfo, published_inf_name), 172);
        assert_eq!(offset_of!(DriverPackageInfo, flags), 692);
        assert_eq!(size_of::<DriverPackageInfo>(), 696);

        assert_eq!(offset_of!(DriverPackageVersionInfo, size), 0);
        assert_eq!(offset_of!(DriverPackageVersionInfo, architecture), 4);
        assert_eq!(offset_of!(DriverPackageVersionInfo, locale_name), 6);
        assert_eq!(offset_of!(DriverPackageVersionInfo, provider_name), 176);
        assert_eq!(offset_of!(DriverPackageVersionInfo, date_low), 696);
        assert_eq!(offset_of!(DriverPackageVersionInfo, version), 704);
        assert_eq!(offset_of!(DriverPackageVersionInfo, class_guid), 712);
        assert_eq!(offset_of!(DriverPackageVersionInfo, class_name), 728);
        assert_eq!(offset_of!(DriverPackageVersionInfo, class_version), 1248);
        assert_eq!(offset_of!(DriverPackageVersionInfo, catalog_file), 1252);
        assert_eq!(offset_of!(DriverPackageVersionInfo, flags), 1772);
        assert_eq!(size_of::<DriverPackageVersionInfo>(), 1776);
    }

    #[test]
    fn values_are_read_only_with_the_right_type_and_size() {
        let mut name = utf16("Intel(R) \u{dc}");
        name.extend_from_slice(&[0, 0]);
        assert_eq!(decode_string(TYPE_STRING, &name).as_deref(), Some("Intel(R) \u{dc}"));
        assert_eq!(decode_string(TYPE_UINT32, &name), None); // wrong type

        assert_eq!(decode_u32(TYPE_UINT32, &0x0D00_0005u32.to_le_bytes()), Some(0x0D00_0005));
        assert_eq!(decode_u32(TYPE_UINT32, &[1, 2]), None); // wrong size
        assert_eq!(decode_u32(TYPE_UINT64, &0x0D00_0005u32.to_le_bytes()), None);
        assert_eq!(decode_u16(TYPE_UINT16, &9u16.to_le_bytes()), Some(9));
        assert_eq!(decode_u16(TYPE_UINT16, &[9]), None);
        assert_eq!(decode_u64(TYPE_UINT64, &5u64.to_le_bytes()), Some(5));
        assert_eq!(decode_filetime(TYPE_FILETIME, &7u64.to_le_bytes()), Some(7));
        assert_eq!(decode_filetime(TYPE_UINT64, &7u64.to_le_bytes()), None);
        assert_eq!(decode_bool(TYPE_BOOLEAN, &[0xFF]), Some(true));
        assert_eq!(decode_bool(TYPE_BOOLEAN, &[0]), Some(false));
        assert_eq!(decode_bool(TYPE_BOOLEAN, &[0, 0]), None);
        assert_eq!(decode_bool(TYPE_STRING, &[1]), None);
    }

    #[test]
    fn guids_and_the_extension_id() {
        // {4d36e972-e325-11ce-bfc1-08002be10318}, the Net class, as Windows keeps it in memory.
        let bytes = [0x72, 0xe9, 0x36, 0x4d, 0x25, 0xe3, 0xce, 0x11, 0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18];
        let guid = decode_guid(TYPE_GUID, &bytes).unwrap();
        assert_eq!(guid.braced(), "{4d36e972-e325-11ce-bfc1-08002be10318}");
        assert_eq!(decode_guid(TYPE_GUID, &bytes[..15]), None);
        assert_eq!(decode_guid(TYPE_STRING, &bytes), None);

        assert_eq!(extension_id_text(Some(guid)), "{4d36e972-e325-11ce-bfc1-08002be10318}");
        assert_eq!(extension_id_text(Some(Guid::default())), ""); // nil: not an extension
        assert_eq!(extension_id_text(None), "");
    }

    #[test]
    fn version_date_and_boot_flags() {
        let version = unpack_version(0x001F_0000_0065_1196);
        assert_eq!(version.to_string(), "31.0.101.4502");

        // 2024-03-05 as a FILETIME, the way DriverVer dates are stored.
        let ticks = (1_709_596_800u64 + 11_644_473_600) * 10_000_000;
        assert_eq!(DateTime::from_filetime(ticks).map(|d| d.date), Date::from_ymd_opt(2024, 3, 5));

        assert_eq!(forced_boot_critical(0), None);
        assert_eq!(forced_boot_critical(FORCE_BOOT_CRITICAL), Some(true));
        assert_eq!(forced_boot_critical(FORCE_NOT_BOOT_CRITICAL), Some(false));
        assert_eq!(forced_boot_critical(0x1), None); // other flags say nothing about it
        assert_eq!(array_text(&[b'o' as u16, b'k' as u16, 0, b'x' as u16]), "ok");
        assert_eq!(array_text(&[b'o' as u16]), "o");
    }
}
