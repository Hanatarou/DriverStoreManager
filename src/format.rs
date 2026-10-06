//! Small pure helpers: sizes, dates, file names, message-box lists, case-insensitive search.

use crate::date::Date;

use crate::culture;

/// Message boxes list at most this many packages; the rest is summarised as "and N more".
pub const MAX_LISTED_PACKAGES: usize = 10;

/// 1536 -> "2 KB", 5242880 -> "5.0 MB". Numbers always use the en-US separators.
pub fn format_size(bytes: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes >= GB {
        format!("{} GB", culture::format_n(bytes / GB, 1))
    } else if bytes >= MB {
        format!("{} MB", culture::format_n(bytes / MB, 1))
    } else if bytes >= KB {
        format!("{} KB", culture::format_n(bytes / KB, 0))
    } else {
        format!("{} B", culture::format_n(bytes, 0))
    }
}

/// 2026-09-03, independent of the Windows language and calendar (Format-Date).
pub fn format_date(date: Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

/// Replaces characters that are not allowed in Windows file names (Get-SafeFileName).
pub fn safe_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let clean = replaced.trim();
    if clean.is_empty() {
        "_".to_string()
    } else {
        clean.to_string()
    }
}

/// [System.IO.Path]::GetFileNameWithoutExtension
pub fn file_name_without_extension(path: &str) -> String {
    let start = path.rfind(['\\', '/', ':']).map(|i| i + 1).unwrap_or(0);
    let name = &path[start..];
    match name.rfind('.') {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

/// Last N lines of a text, used to keep message boxes short (Get-OutputTail).
pub fn output_tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).filter(|l| !l.trim().is_empty()).collect();
    if all.len() <= lines {
        all.join("\n")
    } else {
        format!("...\n{}", all[all.len() - lines..].join("\n"))
    }
}

/// "  oem16.inf (cui_dch.inf)" lines, limited to MAX_LISTED_PACKAGES (Format-PackageList).
pub fn format_package_list(items: &[(String, String)]) -> String {
    let mut lines: Vec<String> =
        items.iter().take(MAX_LISTED_PACKAGES).map(|(published, original)| format!("  {published} ({original})")).collect();
    if items.len() > MAX_LISTED_PACKAGES {
        lines.push(format!("  ... and {} more", items.len() - MAX_LISTED_PACKAGES));
    }
    lines.join("\n")
}

/// Case-insensitive "contains" with [StringComparison]::OrdinalIgnoreCase semantics
/// (each UTF-16 unit is compared after a simple, one-to-one upper-casing).
pub fn contains_ordinal_ignore_case(haystack: &str, needle: &str) -> bool {
    fn fold(s: &str) -> Vec<char> {
        s.chars()
            .map(|c| {
                let mut up = c.to_uppercase();
                match (up.next(), up.next()) {
                    (Some(u), None) => u,
                    _ => c,
                }
            })
            .collect()
    }
    let h = fold(haystack);
    let n = fold(needle);
    if n.is_empty() {
        return true;
    }
    h.windows(n.len()).any(|w| w == n.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formatting() {
        assert_eq!(format_size(0.0), "0 B");
        assert_eq!(format_size(1023.0), "1,023 B");
        assert_eq!(format_size(1536.0), "2 KB");
        assert_eq!(format_size(2560.0), "3 KB"); // 2.5 -> 3 (half away from zero, like .NET)
        assert_eq!(format_size(5242880.0), "5.0 MB");
        assert_eq!(format_size(1310720.0), "1.3 MB"); // 1.25 -> 1.3
        assert_eq!(format_size(1073741824.0), "1.0 GB");
        assert_eq!(format_size(1048575.0), "1,024 KB");
    }

    #[test]
    fn names() {
        assert_eq!(safe_file_name("a:b*c"), "a_b_c");
        assert_eq!(safe_file_name("  "), "_");
        assert_eq!(safe_file_name(" x "), "x");
        assert_eq!(file_name_without_extension("oem16.inf"), "oem16");
        assert_eq!(file_name_without_extension("a.b.inf"), "a.b");
        assert_eq!(file_name_without_extension("noext"), "noext");
        assert_eq!(file_name_without_extension("C:\\x\\y.inf"), "y");
    }

    #[test]
    fn tail() {
        assert_eq!(output_tail("a\r\n\r\nb\r\n", 12), "a\nb");
        let text: String = (1..=20).map(|i| format!("l{i}\n")).collect();
        let out = output_tail(&text, 3);
        assert_eq!(out, "...\nl18\nl19\nl20");
    }

    #[test]
    fn list() {
        let items: Vec<(String, String)> = (1..=12).map(|i| (format!("oem{i}.inf"), format!("x{i}.inf"))).collect();
        let text = format_package_list(&items);
        assert!(text.starts_with("  oem1.inf (x1.inf)\n"));
        assert!(text.ends_with("  ... and 2 more"));
        assert_eq!(text.lines().count(), 11);
    }

    #[test]
    fn contains() {
        assert!(contains_ordinal_ignore_case("Intel(R) Wi-Fi", "wi-fi"));
        assert!(!contains_ordinal_ignore_case("abc", "abd"));
        assert!(contains_ordinal_ignore_case("abc", ""));
    }
}
