//! Language and number rules. The program is English (en-US) only: driver operations are dangerous enough
//! without adding translation problems, so what is shown never depends on the Windows language.
//!
//!  * String sorting and grouping: case-insensitive comparison in the fixed en-US locale
//!    (CompareStringEx with "en-US", NORM_IGNORECASE).
//!  * Numbers ("1,234", "5.0 MB"): always the en-US separators.
//!  * The ONE exception is the field separator of the CSV export: it follows the Windows list separator
//!    (";" in Brazil), because otherwise Excel opens the file as a single column.
//!
//! Dates and times in files and logs are invariant too (see format.rs).

use std::cmp::Ordering;

/// Separators of the current culture (what .NET calls CultureInfo.CurrentCulture.NumberFormat).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberCulture {
    pub decimal_separator: String,
    pub group_separator: String,
    /// .NET NumberGroupSizes: the last element repeats unless it is 0 (0 = no further grouping).
    pub group_sizes: Vec<usize>,
}

impl NumberCulture {
    pub fn invariant() -> Self {
        NumberCulture { decimal_separator: ".".into(), group_separator: ",".into(), group_sizes: vec![3] }
    }

    /// Converts a Win32 LOCALE_SGROUPING string ("3;0", "3;2;0", "3") to .NET group sizes (kept for the tests of
    /// the grouping rules; the program itself always uses the en-US grouping).
    #[cfg(test)]
    pub fn parse_grouping(win32: &str) -> Vec<usize> {
        let mut sizes: Vec<usize> = win32.split(';').filter_map(|p| p.trim().parse().ok()).collect();
        if sizes.is_empty() {
            return vec![0];
        }
        if *sizes.last().unwrap() == 0 {
            // "3;0" means "3, repeated": .NET stores just [3].
            if sizes.len() > 1 {
                sizes.pop();
            }
        } else {
            // "3;2" means "3, then 2, then no more grouping": .NET stores [3, 2, 0].
            sizes.push(0);
        }
        sizes
    }
}

/// 15 significant digits, rounded half away from zero at `decimals` fractional digits.
/// This is how .NET Framework turns a double into "N<decimals>" text (so 1.25 -> "1.3", 2.5 -> "3").
/// Returns (integer digits, fraction digits). Only for finite, non-negative values.
pub fn round_15_digits(value: f64, decimals: usize) -> (String, String) {
    let value = if value.is_finite() && value > 0.0 { value } else { 0.0 };
    if value == 0.0 {
        return ("0".to_string(), "0".repeat(decimals));
    }
    // d.dddddddddddddd e<exp>  (15 significant digits, correctly rounded by Rust)
    let sci = format!("{:.14e}", value);
    let (mantissa, exp) = sci.split_once('e').expect("scientific notation");
    let exp: i32 = exp.parse().expect("exponent");
    let digits: Vec<u8> = mantissa.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();

    // value = 0.d0d1d2... * 10^(exp+1): split into integer and fraction digit vectors.
    let point = exp + 1; // number of integer digits when positive
    let mut int_digits: Vec<u8> = Vec::new();
    let mut frac_digits: Vec<u8> = Vec::new();
    if point <= 0 {
        int_digits.push(0);
        frac_digits.extend(std::iter::repeat(0).take((-point) as usize));
        frac_digits.extend(digits.iter().copied());
    } else {
        for i in 0..point as usize {
            int_digits.push(*digits.get(i).unwrap_or(&0));
        }
        if digits.len() > point as usize {
            frac_digits.extend(digits[point as usize..].iter().copied());
        }
    }

    // Round half up at `decimals` fractional digits.
    let round_up = frac_digits.get(decimals).map(|d| *d >= 5).unwrap_or(false);
    frac_digits.resize(decimals, 0);
    if round_up {
        let mut carry = true;
        for d in frac_digits.iter_mut().rev() {
            if !carry {
                break;
            }
            if *d == 9 {
                *d = 0;
            } else {
                *d += 1;
                carry = false;
            }
        }
        if carry {
            for d in int_digits.iter_mut().rev() {
                if !carry {
                    break;
                }
                if *d == 9 {
                    *d = 0;
                } else {
                    *d += 1;
                    carry = false;
                }
            }
            if carry {
                int_digits.insert(0, 1);
            }
        }
    }
    let to_s = |v: &[u8]| v.iter().map(|d| (b'0' + d) as char).collect::<String>();
    (to_s(&int_digits), to_s(&frac_digits))
}

/// Inserts the group separator into a string of integer digits (like the "N" format).
pub fn group_digits(int_digits: &str, culture: &NumberCulture) -> String {
    let sizes = &culture.group_sizes;
    let digits: Vec<char> = int_digits.chars().collect();
    if sizes.is_empty() || sizes[0] == 0 || digits.len() <= sizes[0] {
        return int_digits.to_string();
    }
    let mut groups: Vec<String> = Vec::new();
    let mut end = digits.len();
    let mut idx = 0usize;
    loop {
        let size = sizes[idx];
        if size == 0 || end <= size {
            groups.push(digits[..end].iter().collect());
            break;
        }
        groups.push(digits[end - size..end].iter().collect());
        end -= size;
        if idx < sizes.len() - 1 {
            idx += 1;
        }
    }
    groups.reverse();
    groups.join(&culture.group_separator)
}

/// '{0:N<decimals>}' -f $value for a non-negative value, with the given culture.
pub fn format_n_with(value: f64, decimals: usize, culture: &NumberCulture) -> String {
    let (int_part, frac_part) = round_15_digits(value, decimals);
    let int_part = group_digits(&int_part, culture);
    if decimals == 0 {
        int_part
    } else {
        format!("{}{}{}", int_part, culture.decimal_separator, frac_part)
    }
}

// --------------------------------------------------------------------------------------------------
// Windows implementation
// --------------------------------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Globalization::{
        CompareStringEx, GetLocaleInfoEx, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, LOCALE_SLIST,
        NORM_IGNORECASE,
    };

    // A null locale name means "the user's locale": only the CSV list separator uses it.
    fn locale_info(kind: u32) -> Option<String> {
        let mut buffer = [0u16; 128];
        let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buffer)) };
        if len <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..(len as usize - 1)]))
    }

    pub fn list_separator() -> String {
        locale_info(LOCALE_SLIST).unwrap_or_else(|| ",".to_string())
    }

    pub fn compare_ignore_case(a: &str, b: &str) -> Ordering {
        let a: Vec<u16> = a.encode_utf16().collect();
        let b: Vec<u16> = b.encode_utf16().collect();
        let result = unsafe {
            CompareStringEx(
                w!("en-US"),
                NORM_IGNORECASE,
                &a,
                &b,
                None,
                None,
                None,
            )
        };
        match result {
            x if x == CSTR_LESS_THAN => Ordering::Less,
            x if x == CSTR_EQUAL => Ordering::Equal,
            x if x == CSTR_GREATER_THAN => Ordering::Greater,
            // The call failed: fall back to an ordinal, case-insensitive comparison.
            _ => a.iter().map(|c| *c).cmp(b.iter().map(|c| *c)),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn list_separator() -> String {
        ",".to_string()
    }
    pub fn compare_ignore_case(a: &str, b: &str) -> Ordering {
        a.to_lowercase().cmp(&b.to_lowercase())
    }
}

/// Case-insensitive string comparison in the fixed en-US locale.
pub fn compare_ignore_case(a: &str, b: &str) -> Ordering {
    imp::compare_ignore_case(a, b)
}

/// Number with en-US separators and `decimals` fractional digits ("1,234.5").
pub fn format_n(value: f64, decimals: usize) -> String {
    format_n_with(value, decimals, &NumberCulture::invariant())
}

/// The Windows list separator (";" in Brazil): the field separator of the CSV export, so Excel splits columns.
pub fn list_separator() -> String {
    imp::list_separator()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(round_15_digits(1.25, 1), ("1".into(), "3".into()));
        assert_eq!(round_15_digits(2.5, 0), ("3".into(), "".into()));
        assert_eq!(round_15_digits(1.5, 0), ("2".into(), "".into()));
        assert_eq!(round_15_digits(0.15, 1), ("0".into(), "2".into()));
        assert_eq!(round_15_digits(5.0, 1), ("5".into(), "0".into()));
        assert_eq!(round_15_digits(9.96, 1), ("10".into(), "0".into()));
        assert_eq!(round_15_digits(999.5, 0), ("1000".into(), "".into()));
        assert_eq!(round_15_digits(0.0, 1), ("0".into(), "0".into()));
        assert_eq!(round_15_digits(0.04, 1), ("0".into(), "0".into()));
        assert_eq!(round_15_digits(1234.0, 0), ("1234".into(), "".into()));
    }

    #[test]
    fn grouping() {
        let inv = NumberCulture::invariant();
        assert_eq!(format_n_with(1234.0, 0, &inv), "1,234");
        assert_eq!(format_n_with(1234567.0, 0, &inv), "1,234,567");
        assert_eq!(format_n_with(999.0, 0, &inv), "999");
        assert_eq!(format_n_with(1536.0 / 1024.0, 0, &inv), "2");
        let br = NumberCulture { decimal_separator: ",".into(), group_separator: ".".into(), group_sizes: vec![3] };
        assert_eq!(format_n_with(1234.5, 1, &br), "1.234,5");
        // Indian grouping: 3;2;0  -> 12,34,567
        let hi = NumberCulture {
            decimal_separator: ".".into(),
            group_separator: ",".into(),
            group_sizes: NumberCulture::parse_grouping("3;2;0"),
        };
        assert_eq!(hi.group_sizes, vec![3, 2]);
        assert_eq!(format_n_with(1234567.0, 0, &hi), "12,34,567");
        // no grouping at all
        let none = NumberCulture { decimal_separator: ".".into(), group_separator: ",".into(), group_sizes: vec![0] };
        assert_eq!(format_n_with(1234567.0, 0, &none), "1234567");
        assert_eq!(NumberCulture::parse_grouping("3;0"), vec![3]);
        assert_eq!(NumberCulture::parse_grouping("3;2"), vec![3, 2, 0]);
    }
}
