//! File-system helpers: folder sizes for the list, the extension ID of an INF, copying a driver package (backup and
//! export) and the count of .inf files used by "Add driver package...". Loops that can take long call
//! `proc::pump_throttled` so the window keeps repainting.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::format::{file_name_without_extension, safe_file_name};
use crate::backup::{self, FileEntry};
use crate::model::Driver;
use crate::proc;

/// Size of a folder for the list: whatever can be read is counted. Returns (bytes, complete). When something
/// cannot be read (a file locked by another program, no permission) the size is a minimum and `complete`
/// is false; the list shows it as "1.2 MB+" and the reason goes to the log through `problems`.
pub fn folder_size_lenient(path: &Path, problems: &mut Vec<String>) -> (u64, bool) {
    let mut total = 0u64;
    let mut complete = true;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(format!("{}: {}", path.display(), clean_io(&error)));
            return (0, false);
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                problems.push(format!("{}: {}", path.display(), clean_io(&error)));
                complete = false;
                continue;
            }
        };
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {
                let (size, ok) = folder_size_lenient(&entry.path(), problems);
                total += size;
                complete &= ok;
            }
            Ok(_) => match entry.metadata() {
                Ok(meta) => total += meta.len(),
                Err(error) => {
                    problems.push(format!("{}: {}", entry.path().display(), clean_io(&error)));
                    complete = false;
                }
            },
            Err(error) => {
                problems.push(format!("{}: {}", entry.path().display(), clean_io(&error)));
                complete = false;
            }
        }
        proc::pump_throttled();
    }
    (total, complete)
}

/// True when two folder paths name the same folder: case-insensitive, trailing "\" or "/" ignored.
pub fn same_folder(a: &str, b: &str) -> bool {
    let normal = |p: &str| p.trim_end_matches(['\\', '/']).to_lowercase();
    normal(a) == normal(b)
}

/// Every .inf file below a folder (hidden names that start with "." are skipped), sorted.
pub fn list_inf_files(folder: &Path) -> io::Result<Vec<PathBuf>> {
    fn walk(folder: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), out)?;
            } else if name.to_lowercase().ends_with(".inf") {
                out.push(entry.path());
            }
            proc::pump_throttled();
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(folder, &mut files)?;
    files.sort();
    Ok(files)
}

/// True when the path is a symbolic link, a junction or any other reparse point.
pub fn is_reparse_point(path: &Path) -> io::Result<bool> {
    let meta = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        Ok(meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(meta.file_type().is_symlink())
    }
}

/// Creates the folder when it is missing and refuses it when it is a link or junction. The program runs
/// elevated: a folder that someone replaced by a junction would redirect what it writes to another place.
pub fn ensure_plain_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| anyhow!("Could not create '{}': {}", path.display(), clean_io(&e)))?;
    if is_reparse_point(path).map_err(|e| anyhow!("Could not check '{}': {}", path.display(), clean_io(&e)))? {
        return Err(anyhow!(
            "'{}' is a link or junction, not a normal folder. It is refused for safety. Remove it or move the program.",
            path.display()
        ));
    }
    Ok(())
}

/// Writes a small file next to its final place and renames it over the target. A link that was planted at
/// the target is replaced by the new file instead of being followed, and a planted temporary file is removed first.
pub fn write_file_safely(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let _ = fs::remove_file(&temporary);
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(anyhow!("Could not write '{}': {}", path.display(), clean_io(&error)));
    }
    Ok(())
}

/// Text of a file the way [System.IO.File]::ReadAllText reads it: UTF-8 / UTF-16 / UTF-32 byte order
/// marks are honoured, everything else is UTF-8.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |data: &[u8], little: bool| -> String {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let utf32 = |data: &[u8], little: bool| -> String {
        data.chunks_exact(4)
            .map(|c| {
                let v = if little { u32::from_le_bytes([c[0], c[1], c[2], c[3]]) } else { u32::from_be_bytes([c[0], c[1], c[2], c[3]]) };
                char::from_u32(v).unwrap_or('\u{FFFD}')
            })
            .collect()
    };
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        utf32(&bytes[4..], true)
    } else if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        utf32(&bytes[4..], false)
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        utf16(&bytes[2..], true)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        utf16(&bytes[2..], false)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Extension INFs declare "ExtensionId = {guid}" in their [Version] section. Packages with different
/// extension IDs are different drivers even when class, provider and INF name match (Get-InfExtensionId).
pub fn extension_id_from_text(text: &str) -> String {
    // Hand-written equivalent of the .NET pattern (?im)^\s*ExtensionId\s*=\s*(\{[0-9a-f\-]+\}):
    // a match may start at the beginning of the text or right after any '\n'.
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let skip_ws = |mut i: usize| {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        i
    };
    let mut starts = vec![0usize];
    starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
    'starts: for start in starts {
        let mut i = skip_ws(start);
        for expected in "extensionid".chars() {
            if i >= n || !chars[i].eq_ignore_ascii_case(&expected) {
                continue 'starts;
            }
            i += 1;
        }
        i = skip_ws(i);
        if i >= n || chars[i] != '=' {
            continue;
        }
        i = skip_ws(i + 1);
        if i >= n || chars[i] != '{' {
            continue;
        }
        let open = i;
        i += 1;
        let body = i;
        while i < n && (chars[i].is_ascii_hexdigit() || chars[i] == '-') {
            i += 1;
        }
        if i == body || i >= n || chars[i] != '}' {
            continue;
        }
        return chars[open..=i].iter().collect::<String>().to_lowercase();
    }
    String::new()
}

pub fn inf_extension_id(inf_path: &Path) -> io::Result<String> {
    let bytes = fs::read(inf_path)?;
    Ok(extension_id_from_text(&decode_text(&bytes)))
}

fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&to)?;
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
        proc::pump_throttled();
    }
    Ok(())
}

/// A package copied by `copy_driver_package`: where it is and the name, size and SHA-256 of each file.
#[derive(Debug)]
pub struct CopiedPackage {
    pub target: PathBuf,
    pub files: Vec<FileEntry>,
}

/// Copies the stored package folder (INF, catalog, binaries) to <DestinationRoot>\<class>\<inf>_<version>_<oemNN>
/// and checks that the copy is identical to the original: the same files, each with the same size and SHA-256.
/// A copy that is not identical is an error. The copy can be added back to the Driver Store with
/// Drivers > Add driver package... or, with the manifest, Drivers > Restore backup...
pub fn copy_driver_package(package: &Driver, destination_root: &Path) -> Result<CopiedPackage> {
    let folder_name = safe_file_name(&format!(
        "{}_{}_{}",
        file_name_without_extension(&package.original_inf),
        package.version,
        file_name_without_extension(&package.published_name)
    ));
    let target = destination_root.join(safe_file_name(&package.class)).join(folder_name);

    // Never mix a new copy with files that are already there: the comparison below would be meaningless.
    if target.exists() {
        return Err(anyhow!("The folder already exists, so nothing was copied: {}", target.display()));
    }

    fs::create_dir_all(&target).map_err(|e| anyhow!("{}", clean_io(&e)))?;
    copy_tree(Path::new(&package.folder), &target).map_err(|e| anyhow!("{}", clean_io(&e)))?;

    let original = backup::hash_tree(Path::new(&package.folder)).map_err(|e| anyhow!("{}", clean_io(&e)))?;
    let copied = backup::hash_tree(&target).map_err(|e| anyhow!("{}", clean_io(&e)))?;
    backup::compare_trees(&original, &copied).map_err(|problem| {
        anyhow!("The copy of {} is not identical to the original ({}): {}", package.published_name, problem, target.display())
    })?;
    Ok(CopiedPackage { target, files: copied })
}

/// io::Error text without the " (os error N)" suffix that Rust appends on Windows.
pub fn clean_io(error: &io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(i) if text.ends_with(')') => text[..i].to_string(),
        _ => text,
    }
}

// ---- .inf files below a folder ----------------------------------------------------------------------
//
// Hidden files are skipped and hidden folders are not entered. The *.inf filter is evaluated by the file
// system (FindFirstFile).

#[cfg(windows)]
mod find {
    use super::*;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES};
    use windows::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileW, FindNextFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, WIN32_FIND_DATAW,
    };

    /// (name, is_directory, is_hidden) of every entry of `directory` matching `pattern`.
    fn entries(directory: &Path, pattern: &str) -> io::Result<Vec<(String, bool, bool)>> {
        let query = directory.join(pattern);
        let mut result = Vec::new();
        let mut data = WIN32_FIND_DATAW::default();
        let handle = match unsafe { FindFirstFileW(&HSTRING::from(query.as_os_str()), &mut data) } {
            Ok(h) => h,
            Err(e) => {
                if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() {
                    return Ok(result);
                }
                return Err(io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32));
            }
        };
        loop {
            let end = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
            let name = String::from_utf16_lossy(&data.cFileName[..end]);
            let is_dir = data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
            let hidden = data.dwFileAttributes & FILE_ATTRIBUTE_HIDDEN.0 != 0;
            result.push((name, is_dir, hidden));
            if let Err(e) = unsafe { FindNextFileW(handle, &mut data) } {
                let _ = unsafe { FindClose(handle) };
                if e.code() == ERROR_NO_MORE_FILES.to_hresult() {
                    return Ok(result);
                }
                return Err(io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32));
            }
        }
    }

    pub fn count_inf_files(folder: &Path) -> io::Result<usize> {
        let mut count = 0usize;
        for (_, is_dir, hidden) in entries(folder, "*.inf")? {
            if !is_dir && !hidden {
                count += 1;
            }
        }
        for (name, is_dir, hidden) in entries(folder, "*")? {
            if is_dir && !hidden && name != "." && name != ".." {
                count += count_inf_files(&folder.join(name))?;
            }
        }
        proc::pump_throttled();
        Ok(count)
    }
}

#[cfg(windows)]
pub use find::count_inf_files;

#[cfg(not(windows))]
pub fn count_inf_files(folder: &Path) -> io::Result<usize> {
    let mut count = 0usize;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() {
            count += count_inf_files(&entry.path())?;
        } else if name.to_lowercase().ends_with(".inf") {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_id() {
        let text = "[Version]\r\nSignature=\"$WINDOWS NT$\"\r\n  ExtensionId = {ABCDEF12-3456-7890-ABCD-EF1234567890}\r\n";
        assert_eq!(extension_id_from_text(text), "{abcdef12-3456-7890-abcd-ef1234567890}");
        assert_eq!(extension_id_from_text("[Version]\nClass=Net\n"), "");
        assert_eq!(extension_id_from_text("; ExtensionId = {aa}\n"), "");
    }

    #[test]
    fn decoding() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFabc"), "abc");
        assert_eq!(decode_text(b"\xFF\xFEa\x00b\x00"), "ab");
        assert_eq!(decode_text(b"\xFE\xFF\x00a\x00b"), "ab");
        assert_eq!(decode_text(b"plain"), "plain");
    }

    #[test]
    fn tree_size_and_copy() {
        let base = std::env::temp_dir().join(format!("dc_test_{}", std::process::id()));
        let src = base.join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.inf"), b"12345").unwrap();
        fs::write(src.join("sub").join("b.sys"), b"1234567890").unwrap();
        assert_eq!(count_inf_files(&src).unwrap(), 1);
        assert!(same_folder("C:\\", "c:") && same_folder("D:\\Img\\", "d:\\img") && !same_folder("C:\\", "D:\\"));

        // Lenient size: a missing folder gives a partial result and a reason, not an error.
        let mut problems = Vec::new();
        assert_eq!(folder_size_lenient(&src, &mut problems), (15, true));
        assert!(problems.is_empty());
        assert_eq!(folder_size_lenient(&base.join("missing"), &mut problems), (0, false));
        assert_eq!(problems.len(), 1);

        // Safe folders and files.
        let plain = base.join("plain");
        ensure_plain_dir(&plain).unwrap();
        assert!(!is_reparse_point(&plain).unwrap());
        // A link planted where the program wants its folder is refused, not followed.
        #[cfg(unix)]
        {
            let link = base.join("planted");
            std::os::unix::fs::symlink(&plain, &link).unwrap();
            assert!(is_reparse_point(&link).unwrap());
            let error = ensure_plain_dir(&link).unwrap_err().to_string();
            assert!(error.contains("link or junction"), "{error}");
        }
        let ini = base.join("settings.ini");
        write_file_safely(&ini, b"one").unwrap();
        write_file_safely(&ini, b"two").unwrap();
        assert_eq!(fs::read(&ini).unwrap(), b"two");
        assert!(!base.join("settings.ini.tmp").exists());

        let package = crate::model::Driver {
            published_name: "oem7.inf".into(),
            number: 7,
            original_inf: "a.inf".into(),
            folder: src.to_string_lossy().to_string(),
            extension_id: String::new(),
            provider: "P".into(),
            class: "Net: x".into(),
            version: crate::netversion::NetVersion::parse("1.0.0.0").unwrap(),
            date: crate::date::Date::from_ymd_opt(2020, 1, 1).unwrap(),
            boot_critical: false,
            signature: crate::model::Signature::Signed,
            size_bytes: 15,
            size_exact: true,
            only_disconnected: false,
            usage_known: true,
            is_old: false,
            status: crate::model::Status::Latest,
            status_text: String::new(),
            in_use: false,
            usage_text: String::new(),
            in_use_text: String::new(),
            device_text: String::new(),
            device_ids: Vec::new(),
            checked: false,
        };
        let dest = base.join("dest");
        let copied = copy_driver_package(&package, &dest).unwrap();
        let target = copied.target;
        assert!(target.ends_with(Path::new("Net_ x").join("a_1.0.0.0_oem7")));
        assert_eq!(copied.files.len(), 2);
        assert_eq!(copied.files.iter().map(|f| f.size).sum::<u64>(), 15);
        assert!(target.join("sub").join("b.sys").exists());

        // The whole chain of a backup: copy -> manifest -> read back -> verify -> detect damage.
        let entry = crate::backup::manifest_package(&package, &dest, &target, copied.files).unwrap();
        assert_eq!(entry.folder, "Net_ x\\a_1.0.0.0_oem7");
        crate::backup::write_manifest(&dest, &[entry]).unwrap();
        let read_back = crate::backup::read_manifest(&dest).unwrap();
        assert!(crate::backup::verify_package(&dest, &read_back[0]).is_ok());
        let original = fs::read(target.join("sub").join("b.sys")).unwrap();
        let mut damaged = original.clone();
        damaged[0] ^= 0xFF; // same size, one byte different
        fs::write(target.join("sub").join("b.sys"), &damaged).unwrap();
        assert!(crate::backup::verify_package(&dest, &read_back[0]).is_err());
        fs::write(target.join("sub").join("b.sys"), &original).unwrap(); // put the content back
        assert!(crate::backup::verify_package(&dest, &read_back[0]).is_ok());

        // Second copy to the same place must fail and say so.
        let err = copy_driver_package(&package, &dest).unwrap_err().to_string();
        assert!(err.starts_with("The folder already exists, so nothing was copied: "));
        fs::remove_dir_all(&base).unwrap();
    }
}

#[cfg(test)]
mod extension_id_tests {
    use super::extension_id_from_text as ext;

    #[test]
    fn matches_the_old_regex_behaviour() {
        assert_eq!(ext("[Version]\r\n ExtensionId = {ABCDEF12-3456-7890-ABCD-EF1234567890}\r\n"), "{abcdef12-3456-7890-abcd-ef1234567890}");
        assert_eq!(ext("extensionid={aa}"), "{aa}");
        assert_eq!(ext("EXTENSIONID\t=\t{A-b}"), "{a-b}");
        assert_eq!(ext("ExtensionId =\r\n{aa}"), "{aa}"); // \s* spans the line break, like in .NET
        assert_eq!(ext("; ExtensionId = {aa}"), "");
        assert_eq!(ext("x ExtensionId = {aa}"), "");
        assert_eq!(ext("ExtensionId = {}"), "");
        assert_eq!(ext("ExtensionId = {zz}"), "");
        assert_eq!(ext("ExtensionId = {aa"), "");
        assert_eq!(ext("ExtensionId {aa}"), "");
        assert_eq!(ext("A=1\nB=2\nExtensionId={f0}\n"), "{f0}");
        assert_eq!(ext(""), "");
    }
}
