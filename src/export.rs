//! File > Export list...: the bytes of the CSV and JSON files (the same layout the original PowerShell
//! version wrote, so files from both versions can be compared or merged).
//!
//!  * CSV  = Export-Csv -NoTypeInformation -Encoding UTF8 -UseCulture
//!           (UTF-8 with BOM, every field quoted, CRLF, the list separator of the current culture).
//!  * JSON = ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8
//!           (UTF-8 with BOM, 4-space indent, `"name":  value`, <, >, & and ' written as \uXXXX,
//!            and a final CRLF added by Set-Content).

use crate::format::format_date;
use crate::model::Driver;

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// CSV column titles match the window columns. The only deliberate differences: "Size" holds bytes here,
/// and "Extension ID" and "Folder" are extras that the window does not show.
const CSV_HEADERS: [&str; 14] = [
    "Published name",
    "Original INF",
    "Provider",
    "Class",
    "Version",
    "Date",
    "Size",
    "Boot-critical",
    "In use",
    "Devices",
    "Status",
    "Extension ID",
    "Folder",
    "Signature",
];

fn csv_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub fn csv_bytes(rows: &[&Driver], separator: &str) -> Vec<u8> {
    let mut text = String::new();
    text.push_str(&CSV_HEADERS.iter().map(|h| csv_quote(h)).collect::<Vec<_>>().join(separator));
    text.push_str("\r\n");
    for p in rows {
        let fields = [
            p.published_name.clone(),
            p.original_inf.clone(),
            p.provider.clone(),
            p.class.clone(),
            p.version.to_string(),
            format_date(p.date),
            p.size_bytes.to_string(),
            if p.boot_critical { "True" } else { "False" }.to_string(),
            p.in_use_text.clone(),
            p.device_text.clone(),
            p.status_text.clone(),
            p.extension_id.clone(),
            p.folder.clone(),
            p.signature.as_str().to_string(),
        ];
        text.push_str(&fields.iter().map(|f| csv_quote(f)).collect::<Vec<_>>().join(separator));
        text.push_str("\r\n");
    }
    let mut bytes = BOM.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// String escaping of ConvertTo-Json in Windows PowerShell 5.1.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{85}' || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            '<' | '>' | '&' | '\'' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn json_bytes(rows: &[&Driver]) -> Vec<u8> {
    let mut text = String::from("[\r\n");
    for (index, p) in rows.iter().enumerate() {
        let properties: [(&str, String); 14] = [
            ("PublishedName", json_string(&p.published_name)),
            ("OriginalInf", json_string(&p.original_inf)),
            ("Provider", json_string(&p.provider)),
            ("Class", json_string(&p.class)),
            ("ExtensionId", json_string(&p.extension_id)),
            ("Version", json_string(&p.version.to_string())),
            ("Date", json_string(&format_date(p.date))),
            ("SizeBytes", p.size_bytes.to_string()),
            ("BootCritical", if p.boot_critical { "true" } else { "false" }.to_string()),
            ("InUse", json_string(&p.in_use_text)),
            ("Devices", json_string(&p.device_text)),
            ("Status", json_string(&p.status_text)),
            ("Folder", json_string(&p.folder)),
            ("Signature", json_string(p.signature.as_str())),
        ];
        text.push_str("    {\r\n");
        for (i, (name, value)) in properties.iter().enumerate() {
            text.push_str(&format!("        \"{}\":  {}", name, value));
            text.push_str(if i + 1 < properties.len() { ",\r\n" } else { "\r\n" });
        }
        text.push_str(if index + 1 < rows.len() { "    },\r\n" } else { "    }\r\n" });
    }
    text.push(']');
    // Set-Content adds a line break after the text.
    text.push_str("\r\n");
    let mut bytes = BOM.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::netversion::NetVersion;
    use crate::date::Date;

    fn sample() -> Driver {
        Driver {
            published_name: "oem1.inf".into(),
            number: 1,
            original_inf: "net<x>.inf".into(),
            folder: "C:\\Windows\\a".into(),
            extension_id: "".into(),
            provider: "Intel \"R\"".into(),
            class: "Net".into(),
            version: NetVersion::parse("1.2.3.4").unwrap(),
            date: Date::from_ymd_opt(2026, 9, 3).unwrap(),
            boot_critical: true,
            signature: Signature::Signed,
            size_bytes: 5242880,
            size_exact: true,
            is_old: false,
            status: Status::Latest,
            status_text: "Latest".into(),
            usage_known: true,
            in_use: false,
            only_disconnected: false,
            usage_text: "Unused".into(),
            in_use_text: "No".into(),
            device_text: "-".into(),
            device_ids: Vec::new(),
            checked: false,
        }
    }

    #[test]
    fn csv_format() {
        let d = sample();
        let bytes = csv_bytes(&[&d], ";");
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        let mut lines = text.split("\r\n");
        assert_eq!(
            lines.next().unwrap(),
            "\"Published name\";\"Original INF\";\"Provider\";\"Class\";\"Version\";\"Date\";\"Size\";\"Boot-critical\";\"In use\";\"Devices\";\"Status\";\"Extension ID\";\"Folder\";\"Signature\""
        );
        assert_eq!(
            lines.next().unwrap(),
            "\"oem1.inf\";\"net<x>.inf\";\"Intel \"\"R\"\"\";\"Net\";\"1.2.3.4\";\"2026-09-03\";\"5242880\";\"True\";\"No\";\"-\";\"Latest\";\"\";\"C:\\Windows\\a\";\"Signed\""
        );
        assert_eq!(lines.next().unwrap(), "");
    }

    #[test]
    fn json_format() {
        let d = sample();
        let bytes = json_bytes(&[&d, &d]);
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        assert!(text.starts_with("[\r\n    {\r\n        \"PublishedName\":  \"oem1.inf\",\r\n"));
        assert!(text.contains("\"OriginalInf\":  \"net\\u003cx\\u003e.inf\""));
        assert!(text.contains("\"Provider\":  \"Intel \\\"R\\\"\""));
        assert!(text.contains("\"SizeBytes\":  5242880,"));
        assert!(text.contains("\"BootCritical\":  true,"));
        assert!(text.contains("\"Folder\":  \"C:\\\\Windows\\\\a\",\r\n        \"Signature\":  \"Signed\"\r\n    },\r\n    {"));
        assert!(text.ends_with("    }\r\n]\r\n"));
    }
}
