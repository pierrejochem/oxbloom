//! `DataSize` and the `Duration` / `DataSize` string parsers used by the binder.

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserialize, Deserializer, Visitor};

/// Splits `s` (trimmed) into digits, an optional `.fraction` and the unit text.
fn split_number(s: &str) -> Option<(&str, &str, &str)> {
    let s = s.trim();
    let int_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (int, rest) = s.split_at(int_end);
    let (frac, unit) = match rest.strip_prefix('.') {
        Some(after) => {
            let end = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            after.split_at(end)
        }
        None => ("", rest),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    Some((int, frac, unit))
}

/// Parses `500ms`, `1.5s`, `2h`; a bare number is seconds. The reason is a fixed text.
pub(crate) fn parse_duration(s: &str) -> Result<Duration, &'static str> {
    const INVALID: &str = "invalid duration";
    let (int, frac, unit) = split_number(s).ok_or(INVALID)?;
    let unit_nanos: u128 = match unit {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" | "" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        _ => return Err("unknown duration unit"),
    };
    const OUT_OF_RANGE: &str = "duration out of range";
    let whole: u128 = if int.is_empty() {
        0
    } else {
        int.parse().map_err(|_| OUT_OF_RANGE)?
    };
    // Digits beyond nanosecond precision of the largest unit carry no information.
    let frac = &frac[..frac.len().min(18)];
    let frac_nanos = if frac.is_empty() {
        0
    } else {
        let digits: u128 = frac.parse().map_err(|_| INVALID)?;
        digits * unit_nanos / 10u128.pow(frac.len() as u32)
    };
    let total = whole
        .checked_mul(unit_nanos)
        .and_then(|n| n.checked_add(frac_nanos))
        .ok_or(OUT_OF_RANGE)?;
    let secs = u64::try_from(total / 1_000_000_000).map_err(|_| OUT_OF_RANGE)?;
    Ok(Duration::new(secs, (total % 1_000_000_000) as u32))
}

/// Parses `512`, `10MB`, `4KiB`; a bare number is bytes. The reason is a fixed text.
pub(crate) fn parse_data_size(s: &str) -> Result<u64, &'static str> {
    const INVALID: &str = "invalid data size";
    let s = s.trim();
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(end);
    if digits.is_empty() {
        return Err(INVALID);
    }
    let factor: u64 = match unit {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return Err("unknown data size unit"),
    };
    let n: u64 = digits.parse().map_err(|_| "data size out of range")?;
    n.checked_mul(factor).ok_or("data size out of range")
}

/// A size in bytes.
///
/// ```
/// use oxbloom_config::DataSize;
///
/// let size = DataSize::new(512);
/// assert_eq!(size.bytes(), 512);
/// assert_eq!(size.to_string(), "512B");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DataSize(u64);

impl DataSize {
    /// A size of `bytes` bytes.
    ///
    /// ```
    /// use oxbloom_config::DataSize;
    ///
    /// assert_eq!(DataSize::new(1024).bytes(), 1024);
    /// ```
    pub fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The size in bytes.
    ///
    /// ```
    /// use oxbloom_config::DataSize;
    ///
    /// assert_eq!(DataSize::new(7).bytes(), 7);
    /// ```
    pub fn bytes(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for DataSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}B", self.0)
    }
}

struct SizeVisitor;

impl Visitor<'_> for SizeVisitor {
    type Value = DataSize;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a data size such as `512`, `10MB` or `4KiB`")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<DataSize, E> {
        parse_data_size(v).map(DataSize).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<DataSize, E> {
        Ok(DataSize(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<DataSize, E> {
        u64::try_from(v)
            .map(DataSize)
            .map_err(|_| E::custom("data size must not be negative"))
    }
}

impl<'de> Deserialize<'de> for DataSize {
    /// Reads a string (`512`, `10MB`, `4KiB`) or an integer (bytes).
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(SizeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::cell::RefCell;

    #[test]
    fn duration_units_parse() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Ok(Duration::from_secs(300)));
        assert_eq!(parse_duration("2h"), Ok(Duration::from_secs(7200)));
        assert_eq!(parse_duration("1d"), Ok(Duration::from_secs(86400)));
        assert_eq!(parse_duration("250us"), Ok(Duration::from_micros(250)));
        assert_eq!(parse_duration("100ns"), Ok(Duration::from_nanos(100)));
        assert_eq!(parse_duration(" 30s "), Ok(Duration::from_secs(30)));
    }

    #[test]
    fn duration_fraction() {
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("0.5m"), Ok(Duration::from_secs(30)));
    }

    #[test]
    fn duration_bare_number_is_seconds() {
        assert_eq!(parse_duration("45"), Ok(Duration::from_secs(45)));
    }

    #[test]
    fn duration_negative_unknown_unit_and_overflow_are_invalid() {
        for bad in ["-1s", "5x", "99999999999999999999d", "", "s", "1 s", "5S"] {
            let reason = parse_duration(bad).expect_err(bad);
            assert!(!reason.contains(bad) || bad.is_empty(), "{reason}");
        }
    }

    #[test]
    fn data_size_decimal_vs_binary() {
        assert_eq!(parse_data_size("1KB"), Ok(1000));
        assert_eq!(parse_data_size("1KiB"), Ok(1024));
        assert_eq!(parse_data_size("2MB"), Ok(2_000_000));
        assert_eq!(parse_data_size("2MiB"), Ok(2 * 1024 * 1024));
        assert_eq!(parse_data_size("1GB"), Ok(1_000_000_000));
        assert_eq!(parse_data_size("1GiB"), Ok(1 << 30));
        assert_eq!(parse_data_size("1TB"), Ok(1_000_000_000_000));
        assert_eq!(parse_data_size("1TiB"), Ok(1 << 40));
        assert_eq!(parse_data_size("10B"), Ok(10));
        assert!(parse_data_size("1kb").is_err());
    }

    #[test]
    fn data_size_bare_number_bytes() {
        assert_eq!(parse_data_size("512"), Ok(512));
        assert_eq!(parse_data_size(" 512 "), Ok(512));
    }

    #[test]
    fn data_size_overflow_invalid() {
        assert!(parse_data_size("18446744073709551616").is_err());
        assert!(parse_data_size("99999999999TiB").is_err());
        assert!(parse_data_size("-1").is_err());
        assert!(parse_data_size("5XB").is_err());
    }

    #[test]
    fn data_size_display_and_plain_serde() {
        assert_eq!(DataSize::new(512).to_string(), "512B");
        #[derive(Deserialize)]
        struct S {
            a: DataSize,
            b: DataSize,
        }
        let v: toml::Value = toml::from_str("a = \"4KiB\"\nb = 7").unwrap();
        let s: S = v.try_into().unwrap();
        assert_eq!((s.a.bytes(), s.b.bytes()), (4096, 7));
    }

    /// Records the arguments serde's `Duration` impl passes to `deserialize_struct`.
    struct Probe<'a>(&'a RefCell<Option<(String, Vec<String>)>>);

    impl<'de> Deserializer<'de> for Probe<'_> {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, _v: V) -> Result<V::Value, Self::Error> {
            Err(de::Error::custom("probe"))
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            name: &'static str,
            fields: &'static [&'static str],
            _v: V,
        ) -> Result<V::Value, Self::Error> {
            *self.0.borrow_mut() = Some((
                name.to_string(),
                fields.iter().map(|f| f.to_string()).collect(),
            ));
            Err(de::Error::custom("probe"))
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
            identifier ignored_any
        }
    }

    #[test]
    fn serde_duration_request_shape_is_pinned() {
        let seen = RefCell::new(None);
        let _ = Duration::deserialize(Probe(&seen));
        assert_eq!(
            seen.into_inner(),
            Some((
                "Duration".to_string(),
                vec!["secs".to_string(), "nanos".to_string()]
            ))
        );
    }
}
