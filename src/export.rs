//! File > Export list...: the bytes of the CSV and JSON files. Both are exact copies of what the window shows:
//! the same columns, in the same order, with the same titles and the same cell text (`row_texts`).
//!
//!  * CSV  = UTF-8 with BOM, every field quoted, CRLF, the list separator of the current culture.
//!  * JSON = UTF-8 with BOM, 4-space indent, `"name":  value`, every value a string, only the escapes the
//!           format requires (so & < > ' stay readable), and a final CRLF.

use crate::model::{row_texts, Driver, COLUMN_DEFINITIONS};

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

fn csv_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Tab-separated text for the clipboard: the column titles, then one line per row, with the same cell text
/// as the window. Pastes into Excel or a text file as it is.
pub fn tsv_text(rows: &[&Driver]) -> String {
    let mut text = COLUMN_DEFINITIONS.iter().map(|c| c.title).collect::<Vec<_>>().join("\t");
    text.push_str("\r\n");
    for p in rows {
        text.push_str(&row_texts(p).join("\t"));
        text.push_str("\r\n");
    }
    text
}

pub fn csv_bytes(rows: &[&Driver], separator: &str) -> Vec<u8> {
    let mut text = String::new();
    text.push_str(&COLUMN_DEFINITIONS.iter().map(|c| csv_quote(c.title)).collect::<Vec<_>>().join(separator));
    text.push_str("\r\n");
    for p in rows {
        text.push_str(&row_texts(p).iter().map(|f| csv_quote(f)).collect::<Vec<_>>().join(separator));
        text.push_str("\r\n");
    }
    let mut bytes = BOM.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// JSON string escaping: only what the format requires (the quote, the backslash and control characters).
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
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn json_bytes(rows: &[&Driver]) -> Vec<u8> {
    let mut text = String::from("[\r\n");
    for (index, p) in rows.iter().enumerate() {
        let properties = row_texts(p);
        text.push_str("    {\r\n");
        for (i, (column, value)) in COLUMN_DEFINITIONS.iter().zip(properties.iter()).enumerate() {
            text.push_str(&format!("        {}:  {}", json_string(column.title), json_string(value)));
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
            signature: Signature(0x0D00_0005),
            signer: "Microsoft Windows Hardware Compatibility Publisher".into(),
            install_date: None,
            size_bytes: 5242880,
            size_exact: true,
            files: vec!["net.inf".into(), "net.sys".into()],
            is_old: false,
            status: Status::Latest,
            status_text: "Latest".into(),
            usage_known: true,
            in_use: false,
            only_disconnected: false,
            has_problem_device: false,
            usage_text: "Unused".into(),
            in_use_text: "No".into(),
            device_text: "-".into(),
            device_ids: Vec::new(),
            absent_devices: Vec::new(),
            checked: false,
            protected: false,
        }
    }

    #[test]
    fn csv_is_the_window() {
        let d = sample();
        let bytes = csv_bytes(&[&d], ";");
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        let mut lines = text.split("\r\n");
        let titles: Vec<String> = COLUMN_DEFINITIONS.iter().map(|c| format!("\"{}\"", c.title)).collect();
        assert_eq!(lines.next().unwrap(), titles.join(";"));
        let cells: Vec<String> = row_texts(&d).iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect();
        assert_eq!(lines.next().unwrap(), cells.join(";"));
        assert_eq!(lines.next().unwrap(), "");
        assert!(text.contains("\"Install date (UTC)\";"));
        assert!(text.contains("\"Device ID\";"));
    }

    #[test]
    fn tsv_is_the_window() {
        let d = sample();
        let text = tsv_text(&[&d, &d]);
        let mut lines = text.split("\r\n");
        let titles: Vec<&str> = COLUMN_DEFINITIONS.iter().map(|c| c.title).collect();
        assert_eq!(lines.next().unwrap(), titles.join("\t"));
        let cells = row_texts(&d).join("\t");
        assert_eq!(lines.next().unwrap(), cells);
        assert_eq!(lines.next().unwrap(), cells);
        assert_eq!(lines.next().unwrap(), "");
    }

    #[test]
    fn json_is_the_window() {
        let d = sample();
        let bytes = json_bytes(&[&d, &d]);
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        assert!(text.starts_with("[\r\n    {\r\n        \"Published name\":  \"oem1.inf\",\r\n"));
        assert!(text.contains("\"Original INF\":  \"net<x>.inf\""));
        assert!(text.contains("\"Provider\":  \"Intel \\\"R\\\"\""));
        assert!(text.contains("\"Boot-critical\":  \"Yes\","));
        assert!(text.contains("\"Install date (UTC)\":  \"-\","));
        assert!(text.contains("\"Driver path\":  \"C:\\\\Windows\\\\a\"\r\n    },\r\n    {"));
        assert!(text.ends_with("    }\r\n]\r\n"));
    }
}
