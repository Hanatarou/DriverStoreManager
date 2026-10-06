//! Verified copies and the backup manifest.
//!
//!  * SHA-256 in plain Rust (no crate, no Windows call), checked against the published test vectors.
//!  * `hash_tree` / `compare_trees`: a backup or an export is only accepted when every file of the copy has
//!    the same name, size and SHA-256 as the original (the size alone does not catch a damaged copy).
//!  * `manifest.txt`: written inside every backup / export folder. It records each package, the devices that
//!    used it and the SHA-256 of every file, so "Drivers > Restore backup..." can check that the backup is
//!    intact before it installs anything.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::fsops::{clean_io, write_file_safely};
use crate::model::Driver;
use crate::native::looks_like_published_name;
use crate::proc;

/// Name of the manifest file inside a backup or export folder.
pub const MANIFEST_NAME: &str = "manifest.txt";
/// Largest manifest that is read.
pub const MANIFEST_MAX_BYTES: u64 = 16 * 1024 * 1024;

// ---- SHA-256 ----------------------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    total: u64,
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            block: [0; 64],
            filled: 0,
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                let block = self.block;
                compress(&mut self.state, &block);
                self.filled = 0;
            }
        }
        while data.len() >= 64 {
            let block: &[u8; 64] = data[..64].try_into().expect("64 bytes");
            compress(&mut self.state, block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.filled = data.len();
        }
    }

    /// The 32-byte digest as 64 lower-case hex digits.
    pub fn finish_hex(mut self) -> String {
        let bit_length = self.total.wrapping_mul(8);
        let mut tail = [0u8; 128];
        tail[..self.filled].copy_from_slice(&self.block[..self.filled]);
        tail[self.filled] = 0x80;
        let blocks = if self.filled < 56 { 1 } else { 2 };
        let end = blocks * 64;
        tail[end - 8..end].copy_from_slice(&bit_length.to_be_bytes());
        for i in 0..blocks {
            let block: &[u8; 64] = tail[i * 64..(i + 1) * 64].try_into().expect("64 bytes");
            compress(&mut self.state, block);
        }
        self.state.iter().map(|word| format!("{word:08x}")).collect()
    }
}

/// SHA-256 and size of a file. The window keeps repainting while a big file is read.
pub fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
        proc::pump_throttled();
    }
    Ok((hasher.finish_hex(), size))
}

// ---- Trees of files -----------------------------------------------------------------------------------

/// One file of a package: its path below the package folder (always with "\"), its size and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub relative: String,
    pub size: u64,
    pub sha256: String,
}

/// root + a relative path written with "\" (works on any platform).
pub fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('\\') {
        path.push(part);
    }
    path
}

fn collect_files(root: &Path, relative: &[String], out: &mut Vec<Vec<String>>) -> io::Result<()> {
    let mut folder = root.to_path_buf();
    for part in relative {
        folder.push(part);
    }
    for entry in fs::read_dir(&folder)? {
        let entry = entry?;
        let mut path = relative.to_vec();
        path.push(entry.file_name().to_string_lossy().to_string());
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Name, size and SHA-256 of every file below `root`, sorted by path.
pub fn hash_tree(root: &Path) -> io::Result<Vec<FileEntry>> {
    let mut paths = Vec::new();
    collect_files(root, &[], &mut paths)?;
    let mut entries = Vec::with_capacity(paths.len());
    for parts in paths {
        let relative = parts.join("\\");
        let (sha256, size) = hash_file(&join_relative(root, &relative))?;
        entries.push(FileEntry { relative, size, sha256 });
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(entries)
}

/// Ok when both lists have the same files with the same size and SHA-256; otherwise the first problems.
pub fn compare_trees(expected: &[FileEntry], actual: &[FileEntry]) -> Result<(), String> {
    let actual_by_name: HashMap<&str, &FileEntry> = actual.iter().map(|f| (f.relative.as_str(), f)).collect();
    let mut problems: Vec<String> = Vec::new();
    for file in expected {
        match actual_by_name.get(file.relative.as_str()) {
            None => problems.push(format!("missing: {}", file.relative)),
            Some(found) if found.size != file.size => {
                problems.push(format!("different size ({} instead of {} bytes): {}", found.size, file.size, file.relative))
            }
            Some(found) if found.sha256 != file.sha256 => problems.push(format!("different content: {}", file.relative)),
            Some(_) => {}
        }
    }
    let expected_names: std::collections::HashSet<&str> = expected.iter().map(|f| f.relative.as_str()).collect();
    for file in actual {
        if !expected_names.contains(file.relative.as_str()) {
            problems.push(format!("unexpected file: {}", file.relative));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    let more = if problems.len() > 3 { format!(" (and {} more)", problems.len() - 3) } else { String::new() };
    problems.truncate(3);
    Err(format!("{}{}", problems.join("; "), more))
}

// ---- Manifest -------------------------------------------------------------------------------------------

/// One package in a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestPackage {
    pub published_name: String,
    pub inf: String,
    pub class: String,
    pub provider: String,
    pub version: String,
    pub date: String,
    /// Folder of the package below the backup folder, with "\".
    pub folder: String,
    /// Instance IDs of the devices that used the package when it was backed up.
    pub devices: Vec<String>,
    pub files: Vec<FileEntry>,
}

fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

/// A relative path that stays inside its folder: not empty, no drive, no leading or double separator, no "..".
pub fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(':')
        && !path.contains('/')
        && !path.chars().any(|c| c.is_control())
        && path.split('\\').all(|part| !part.is_empty() && part != "." && part != "..")
}

pub fn manifest_text(packages: &[ManifestPackage]) -> String {
    let mut text = String::from("; DriverStore Manager backup manifest, version 1. Used by Drivers > Restore backup.\r\n");
    for p in packages {
        text.push_str("[Package]\r\n");
        text.push_str(&format!("Published={}\r\n", one_line(&p.published_name)));
        text.push_str(&format!("Inf={}\r\n", one_line(&p.inf)));
        text.push_str(&format!("Class={}\r\n", one_line(&p.class)));
        text.push_str(&format!("Provider={}\r\n", one_line(&p.provider)));
        text.push_str(&format!("Version={}\r\n", one_line(&p.version)));
        text.push_str(&format!("Date={}\r\n", one_line(&p.date)));
        text.push_str(&format!("Folder={}\r\n", one_line(&p.folder)));
        for device in &p.devices {
            text.push_str(&format!("Device={}\r\n", one_line(device)));
        }
        for file in &p.files {
            text.push_str(&format!("File={}|{}|{}\r\n", file.sha256, file.size, one_line(&file.relative)));
        }
    }
    text
}

fn finish_package(current: Option<ManifestPackage>, packages: &mut Vec<ManifestPackage>) -> Result<(), String> {
    let Some(p) = current else { return Ok(()) };
    let name = if p.published_name.is_empty() { "(no name)".to_string() } else { p.published_name.clone() };
    if !looks_like_published_name(&p.published_name) {
        return Err(format!("{name}: the published name is not oemNN.inf"));
    }
    if p.inf.is_empty() || p.inf.contains('\\') || p.inf.contains('/') || p.inf.contains(':') || !p.inf.to_ascii_lowercase().ends_with(".inf") {
        return Err(format!("{name}: the INF name '{}' is not valid", p.inf));
    }
    if !is_safe_relative(&p.folder) {
        return Err(format!("{name}: the folder '{}' is not valid", p.folder));
    }
    if p.files.is_empty() {
        return Err(format!("{name}: the package has no files"));
    }
    if !p.files.iter().any(|f| f.relative.eq_ignore_ascii_case(&p.inf)) {
        return Err(format!("{name}: the INF file {} is not in the list of files", p.inf));
    }
    packages.push(p);
    Ok(())
}

/// Reads a manifest. Strict about everything that is later used as a path or a command argument.
pub fn parse_manifest(text: &str) -> Result<Vec<ManifestPackage>, String> {
    let mut packages: Vec<ManifestPackage> = Vec::new();
    let mut current: Option<ManifestPackage> = None;
    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r').trim_start_matches('\u{feff}');
        if line.trim().is_empty() || line.starts_with(';') {
            continue;
        }
        if line == "[Package]" {
            finish_package(current.take(), &mut packages)?;
            current = Some(ManifestPackage {
                published_name: String::new(),
                inf: String::new(),
                class: String::new(),
                provider: String::new(),
                version: String::new(),
                date: String::new(),
                folder: String::new(),
                devices: Vec::new(),
                files: Vec::new(),
            });
            continue;
        }
        let Some(package) = current.as_mut() else { continue };
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {} is not 'Key=Value'", number + 1));
        };
        match key {
            "Published" => package.published_name = value.to_string(),
            "Inf" => package.inf = value.to_string(),
            "Class" => package.class = value.to_string(),
            "Provider" => package.provider = value.to_string(),
            "Version" => package.version = value.to_string(),
            "Date" => package.date = value.to_string(),
            "Folder" => package.folder = value.to_string(),
            "Device" => {
                if !value.is_empty() {
                    package.devices.push(value.to_string());
                }
            }
            "File" => {
                let mut parts = value.splitn(3, '|');
                let (Some(sha), Some(size), Some(relative)) = (parts.next(), parts.next(), parts.next()) else {
                    return Err(format!("line {}: a File entry needs 'sha256|size|path'", number + 1));
                };
                let size: u64 = size.parse().map_err(|_| format!("line {}: the size '{}' is not a number", number + 1, size))?;
                if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                    return Err(format!("line {}: the SHA-256 is not 64 hex digits", number + 1));
                }
                if !is_safe_relative(relative) {
                    return Err(format!("line {}: the path '{}' is not valid", number + 1, relative));
                }
                package.files.push(FileEntry { relative: relative.to_string(), size, sha256: sha.to_string() });
            }
            _ => {}
        }
    }
    finish_package(current.take(), &mut packages)?;
    if packages.is_empty() {
        return Err("it lists no packages".to_string());
    }
    Ok(packages)
}

/// The manifest entry for a package that was just copied to `target` (a folder below `session_root`).
pub fn manifest_package(
    package: &Driver,
    session_root: &Path,
    target: &Path,
    files: Vec<FileEntry>,
) -> Result<ManifestPackage, String> {
    let relative = target.strip_prefix(session_root).map_err(|_| "the copy is outside the backup folder".to_string())?;
    let folder = relative.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("\\");
    Ok(ManifestPackage {
        published_name: package.published_name.clone(),
        inf: package.original_inf.clone(),
        class: package.class.clone(),
        provider: package.provider.clone(),
        version: package.version.to_string(),
        date: crate::format::format_date(package.date),
        folder,
        devices: package.device_ids.clone(),
        files,
    })
}

/// Writes (replaces) the manifest of a backup or export folder.
pub fn write_manifest(session_root: &Path, packages: &[ManifestPackage]) -> anyhow::Result<()> {
    write_file_safely(&session_root.join(MANIFEST_NAME), manifest_text(packages).as_bytes())
}

/// Reads and parses the manifest of a backup folder.
pub fn read_manifest(session_root: &Path) -> Result<Vec<ManifestPackage>, String> {
    let path = session_root.join(MANIFEST_NAME);
    let meta = fs::metadata(&path).map_err(|e| format!("{}: {}", path.display(), clean_io(&e)))?;
    if meta.len() > MANIFEST_MAX_BYTES {
        return Err("the manifest is too large".to_string());
    }
    let bytes = fs::read(&path).map_err(|e| format!("{}: {}", path.display(), clean_io(&e)))?;
    let text = String::from_utf8(bytes).map_err(|_| "the manifest is not valid UTF-8 text".to_string())?;
    parse_manifest(&text)
}

/// Checks that the files of a package in a backup folder are exactly the ones in the manifest.
pub fn verify_package(session_root: &Path, package: &ManifestPackage) -> Result<(), String> {
    let root = join_relative(session_root, &package.folder);
    let actual = hash_tree(&root).map_err(|e| format!("{}: {}", root.display(), clean_io(&e)))?;
    compare_trees(&package.files, &actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha_of(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finish_hex()
    }

    #[test]
    fn sha256_published_vectors() {
        assert_eq!(sha_of(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha_of(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            sha_of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(sha_of(&million), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn sha256_does_not_depend_on_how_the_data_is_split() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let whole = sha_of(&data);
        for chunk in [1usize, 3, 55, 56, 57, 63, 64, 65, 127, 500] {
            let mut hasher = Sha256::new();
            for piece in data.chunks(chunk) {
                hasher.update(piece);
            }
            assert_eq!(hasher.finish_hex(), whole, "chunk size {chunk}");
        }
        // Lengths around the padding boundary (55, 56, 63, 64 bytes).
        assert_eq!(sha_of(&[b'a'; 55]), "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318");
        assert_eq!(sha_of(&[b'a'; 56]), "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a");
        assert_eq!(sha_of(&[b'a'; 64]), "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsm_backup_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn trees_are_hashed_and_compared() {
        let base = temp_dir("tree");
        let a = base.join("a");
        let b = base.join("b");
        for root in [&a, &b] {
            fs::create_dir_all(root.join("sub")).unwrap();
            fs::write(root.join("x.inf"), b"inf").unwrap();
            fs::write(root.join("sub").join("y.sys"), b"binary").unwrap();
        }
        let first = hash_tree(&a).unwrap();
        assert_eq!(first.iter().map(|f| f.relative.as_str()).collect::<Vec<_>>(), vec!["sub\\y.sys", "x.inf"]);
        assert_eq!(first[1].sha256, sha_of(b"inf"));
        assert_eq!(first[1].size, 3);
        assert!(compare_trees(&first, &hash_tree(&b).unwrap()).is_ok());

        // Same size, different content: only the hash notices.
        fs::write(b.join("sub").join("y.sys"), b"binarz").unwrap();
        let error = compare_trees(&first, &hash_tree(&b).unwrap()).unwrap_err();
        assert!(error.contains("different content: sub\\y.sys"), "{error}");
        // Missing and extra files.
        fs::remove_file(b.join("x.inf")).unwrap();
        fs::write(b.join("z.txt"), b"z").unwrap();
        let error = compare_trees(&first, &hash_tree(&b).unwrap()).unwrap_err();
        assert!(error.contains("missing: x.inf") && error.contains("unexpected file: z.txt"), "{error}");
        let _ = fs::remove_dir_all(&base);
    }

    fn sample_package() -> ManifestPackage {
        ManifestPackage {
            published_name: "oem56.inf".into(),
            inf: "heci.inf".into(),
            class: "System".into(),
            provider: "Intel".into(),
            version: "2552.8.10.0".into(),
            date: "2025-12-23".into(),
            folder: "System\\heci_2552.8.10.0_oem56".into(),
            devices: vec!["PCI\\VEN_8086&DEV_7A68\\3&11583659&0&B0".into()],
            files: vec![
                FileEntry { relative: "heci.inf".into(), size: 100, sha256: sha_of(b"one") },
                FileEntry { relative: "x64\\heci.sys".into(), size: 2000, sha256: sha_of(b"two") },
            ],
        }
    }

    #[test]
    fn manifest_round_trip() {
        let packages = vec![sample_package(), ManifestPackage { published_name: "oem7.inf".into(), ..sample_package() }];
        let parsed = parse_manifest(&manifest_text(&packages)).unwrap();
        assert_eq!(parsed, packages);
    }

    #[test]
    fn manifest_is_strict_about_paths_and_names() {
        let good = manifest_text(&[sample_package()]);
        assert!(parse_manifest(&good).is_ok());
        for (from, to) in [
            ("Folder=System\\heci_2552.8.10.0_oem56", "Folder=..\\..\\Windows"),
            ("Folder=System\\heci_2552.8.10.0_oem56", "Folder=C:\\Windows"),
            ("Folder=System\\heci_2552.8.10.0_oem56", "Folder=\\System"),
            ("Published=oem56.inf", "Published=evil.inf"),
            ("Inf=heci.inf", "Inf=..\\heci.inf"),
            ("|x64\\heci.sys", "|..\\heci.sys"),
        ] {
            let bad = good.replace(from, to);
            assert!(parse_manifest(&bad).is_err(), "{to}");
        }
        // A sha256 that is not hex, a size that is not a number, no INF among the files, nothing at all.
        assert!(parse_manifest(&good.replacen(&sha_of(b"one"), &"g".repeat(64), 1)).is_err());
        assert!(parse_manifest(&good.replace("|100|", "|abc|")).is_err());
        assert!(parse_manifest(&good.replace("|100|heci.inf", "|100|other.txt")).is_err());
        assert!(parse_manifest("; only a comment\r\n").is_err());
        assert!(is_safe_relative("a\\b.sys") && !is_safe_relative("a\\\\b") && !is_safe_relative("a/b") && !is_safe_relative(""));
    }

    #[test]
    fn verifies_a_package_in_a_backup_folder() {
        let base = temp_dir("verify");
        let package_dir = join_relative(&base, "System\\heci_x_oem56");
        fs::create_dir_all(package_dir.join("x64")).unwrap();
        fs::write(package_dir.join("heci.inf"), b"one").unwrap();
        fs::write(package_dir.join("x64").join("heci.sys"), b"two").unwrap();
        let files = hash_tree(&package_dir).unwrap();
        let package = ManifestPackage { folder: "System\\heci_x_oem56".into(), files, ..sample_package() };
        assert!(verify_package(&base, &package).is_ok());
        fs::write(package_dir.join("x64").join("heci.sys"), b"twx").unwrap();
        assert!(verify_package(&base, &package).unwrap_err().contains("different content"));

        // The manifest file is written and read back.
        write_manifest(&base, &[package.clone()]).unwrap();
        assert_eq!(read_manifest(&base).unwrap(), vec![package]);
        let _ = fs::remove_dir_all(&base);
    }
}
