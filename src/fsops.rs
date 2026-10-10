//! File-system helpers: folder sizes and file names for the list, copying a driver package (backup and
//! export) and the count of .inf files used by "Add driver package...". Loops that can take long call
//! `proc::pump_throttled` so the window keeps repainting.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::culture::compare_ignore_case;
use crate::format::{file_name_without_extension, safe_file_name};
use crate::backup::{self, FileEntry};
use crate::model::Driver;
use crate::proc;

/// What a package folder holds, for the list.
#[derive(Debug, PartialEq, Eq)]
pub struct FolderScan {
    pub bytes: u64,
    /// False when something could not be read: `bytes` is then a minimum (and `files` may be missing some names).
    pub complete: bool,
    /// The names of the files, relative to the folder ("sub\b.sys"), sorted.
    pub files: Vec<String>,
}

/// Size and file names of a folder for the list: whatever can be read is counted. When something cannot be
/// read (a file locked by another program, no permission) the size is a minimum and `complete` is false; the
/// list shows it as "1.2 MB+" and the reason goes to the log through `problems`.
pub fn scan_folder(path: &Path, problems: &mut Vec<String>) -> FolderScan {
    let mut scan = FolderScan { bytes: 0, complete: true, files: Vec::new() };
    scan_into(path, "", problems, &mut scan);
    scan.files.sort_by(|a, b| compare_ignore_case(a, b));
    scan
}

fn scan_into(path: &Path, prefix: &str, problems: &mut Vec<String>, scan: &mut FolderScan) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(format!("{}: {}", path.display(), clean_io(&error)));
            scan.complete = false;
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                problems.push(format!("{}: {}", path.display(), clean_io(&error)));
                scan.complete = false;
                continue;
            }
        };
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => scan_into(&entry.path(), &format!("{name}\\"), problems, scan),
            Ok(_) => match entry.metadata() {
                Ok(meta) => {
                    scan.bytes += meta.len();
                    scan.files.push(name);
                }
                Err(error) => {
                    problems.push(format!("{}: {}", entry.path().display(), clean_io(&error)));
                    scan.complete = false;
                }
            },
            Err(error) => {
                problems.push(format!("{}: {}", entry.path().display(), clean_io(&error)));
                scan.complete = false;
            }
        }
        proc::pump_throttled();
    }
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
    fn tree_size_and_copy() {
        let base = std::env::temp_dir().join(format!("dc_test_{}", std::process::id()));
        let src = base.join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.inf"), b"12345").unwrap();
        fs::write(src.join("sub").join("b.sys"), b"1234567890").unwrap();
        assert_eq!(count_inf_files(&src).unwrap(), 1);
        assert!(same_folder("C:\\", "c:") && same_folder("D:\\Img\\", "d:\\img") && !same_folder("C:\\", "D:\\"));

        // Lenient scan: size and file names (sorted, with the sub folder); a missing folder gives a partial
        // result and a reason, not an error.
        let mut problems = Vec::new();
        let scan = scan_folder(&src, &mut problems);
        assert_eq!((scan.bytes, scan.complete), (15, true));
        assert_eq!(scan.files, vec!["a.inf".to_string(), "sub\\b.sys".to_string()]);
        assert!(problems.is_empty());
        let missing = scan_folder(&base.join("missing"), &mut problems);
        assert_eq!(missing, FolderScan { bytes: 0, complete: false, files: Vec::new() });
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
            signature: crate::model::Signature(0x0D00_0005),
            signer: String::new(),
            install_date: None,
            size_bytes: 15,
            size_exact: true,
            files: Vec::new(),
            only_disconnected: false,
            has_problem_device: false,
            usage_known: true,
            is_old: false,
            status: crate::model::Status::Latest,
            status_text: String::new(),
            in_use: false,
            usage_text: String::new(),
            in_use_text: String::new(),
            device_text: String::new(),
            device_ids: Vec::new(),
            absent_devices: Vec::new(),
            checked: false,
            protected: false,
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
