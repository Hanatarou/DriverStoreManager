//! Calendar dates and the local clock, without a date/time crate.
//!
//! `Date` is a calendar day, ordered by year, month, day.
//! Times in file names and logs always use the same digits on every Windows (invariant culture).

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        _ => 28,
    }
}

impl Date {
    pub fn from_ymd_opt(year: i32, month: u32, day: u32) -> Option<Date> {
        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// Parses "yyyy-MM-dd" (tests only).
    #[cfg(test)]
    pub fn parse_iso(text: &str) -> Option<Date> {
        let mut parts = text.split('-');
        let year = parts.next()?.parse::<i32>().ok()?;
        let month = parts.next()?.parse::<u32>().ok()?;
        let day = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Date::from_ymd_opt(year, month, day)
    }
}

pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

#[cfg(windows)]
pub fn local_now() -> LocalTime {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
    }
}

/// Not used by the program (it only runs on Windows); lets the tests run anywhere (UTC).
#[cfg(not(windows))]
pub fn local_now() -> LocalTime {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + if month <= 2 { 1 } else { 0 }) as i32;
    LocalTime { year, month, day, hour: (rest / 3600) as u32, minute: (rest % 3600 / 60) as u32, second: (rest % 60) as u32 }
}

/// The only two layouts the program uses: "%Y%m%d_%H%M%S" and "%Y-%m-%d %H:%M:%S".
pub fn format_now(layout: &str) -> String {
    let t = local_now();
    let mut out = String::new();
    let mut chars = layout.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{:04}", t.year)),
            Some('m') => out.push_str(&format!("{:02}", t.month)),
            Some('d') => out.push_str(&format!("{:02}", t.day)),
            Some('H') => out.push_str(&format!("{:02}", t.hour)),
            Some('M') => out.push_str(&format!("{:02}", t.minute)),
            Some('S') => out.push_str(&format!("{:02}", t.second)),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert!(Date::from_ymd_opt(2024, 2, 29).is_some());
        assert!(Date::from_ymd_opt(2023, 2, 29).is_none());
        assert!(Date::from_ymd_opt(2024, 13, 1).is_none());
        assert_eq!(Date::parse_iso("2026-09-03"), Date::from_ymd_opt(2026, 9, 3));
        assert_eq!(Date::parse_iso("2026-9-3"), Date::from_ymd_opt(2026, 9, 3));
        assert_eq!(Date::parse_iso("2026-09"), None);
        assert_eq!(Date::parse_iso("2026-09-03-1"), None);
        assert_eq!(Date::parse_iso("x"), None);
        assert!(Date::from_ymd_opt(2020, 12, 31) < Date::from_ymd_opt(2021, 1, 1));
        assert!(Date::from_ymd_opt(2021, 2, 1) > Date::from_ymd_opt(2021, 1, 31));
    }

    #[test]
    fn clock_layouts() {
        let stamp = format_now("%Y%m%d_%H%M%S");
        assert_eq!(stamp.len(), 15);
        assert_eq!(stamp.as_bytes()[8], b'_');
        let line = format_now("%Y-%m-%d %H:%M:%S");
        assert_eq!(line.len(), 19);
        assert_eq!(&line[4..5], "-");
        assert_eq!(&line[13..14], ":");
    }
}
