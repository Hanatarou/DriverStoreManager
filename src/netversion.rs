//! System.Version semantics (what `[version]"1.2.3.4"` is in the PowerShell script).
//!
//! Components that are not present are -1, exactly like in .NET, so "1.2" < "1.2.0" < "1.2.0.0".
//! The derived ordering compares major, minor, build, revision in that order, like Version.CompareTo.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NetVersion {
    pub major: i32,
    pub minor: i32,
    pub build: i32,
    pub revision: i32,
}

impl NetVersion {
    /// Like `new Version(string)`: 2 to 4 non-negative Int32 components separated by dots (tests only: the
    /// program gets the four numbers from the Driver Store directly).
    #[cfg(test)]
    pub fn parse(text: &str) -> Result<NetVersion, String> {
        let fail = || {
            format!(
                "Cannot convert value \"{text}\" to type \"System.Version\". \
                 Error: \"Version string portion was too short or too long.\""
            )
        };
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() < 2 || parts.len() > 4 {
            return Err(fail());
        }
        let mut numbers = [-1i32; 4];
        for (i, part) in parts.iter().enumerate() {
            let value: i32 = part.trim().parse().map_err(|_| {
                format!(
                    "Cannot convert value \"{text}\" to type \"System.Version\". \
                     Error: \"Input string was not in a correct format.\""
                )
            })?;
            if value < 0 {
                return Err(format!(
                    "Cannot convert value \"{text}\" to type \"System.Version\". \
                     Error: \"Version's parameters must be greater than or equal to zero.\""
                ));
            }
            numbers[i] = value;
        }
        Ok(NetVersion { major: numbers[0], minor: numbers[1], build: numbers[2], revision: numbers[3] })
    }
}

impl fmt::Display for NetVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.build < 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else if self.revision < 0 {
            write!(f, "{}.{}.{}", self.major, self.minor, self.build)
        } else {
            write!(f, "{}.{}.{}.{}", self.major, self.minor, self.build, self.revision)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let v = NetVersion::parse("31.0.101.4502").unwrap();
        assert_eq!(v.to_string(), "31.0.101.4502");
        assert_eq!(NetVersion::parse("1.2").unwrap().to_string(), "1.2");
        assert_eq!(NetVersion::parse("1.2.3").unwrap().to_string(), "1.2.3");
        assert!(NetVersion::parse("1").is_err());
        assert!(NetVersion::parse("1.2.3.4.5").is_err());
        assert!(NetVersion::parse("1.x").is_err());
        assert!(NetVersion::parse("1.-2").is_err());
    }

    #[test]
    fn ordering_matches_dotnet() {
        let p = |s| NetVersion::parse(s).unwrap();
        assert!(p("1.2") < p("1.2.0"));
        assert!(p("1.2.0") < p("1.2.0.0"));
        assert!(p("1.10.0.0") > p("1.9.0.0"));
        assert!(p("2.0.0.0") > p("1.99.99.99"));
        assert_eq!(p("1.2.3.4"), p("1.2.3.4"));
    }
}
