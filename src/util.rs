//! Small helpers shared by everything: errors, time, paths.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// An error whose message is meant to be shown to the user as is.
#[derive(Debug, Clone)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// Parse `2026-11-10T06:19:56.638Z` (or with a `+HH:MM` offset) to epoch milliseconds.
pub fn iso_to_ms(value: &str) -> Option<i64> {
    let b = value.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |from: usize, to: usize| value.get(from..to)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &value[19..];
    let mut millis = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        rest = &frac[digits.len()..];
        let padded = format!("{:0<3}", &digits[..digits.len().min(3)]);
        millis = padded.parse::<i64>().ok()?;
    }
    let offset_min = match rest {
        "" | "Z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hh = rest.get(1..3)?.parse::<i64>().ok()?;
            let mm = rest.get(rest.len().checked_sub(2)?..)?.parse::<i64>().ok()?;
            sign * (hh * 60 + mm)
        }
    };
    let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s - offset_min * 60;
    Some(secs * 1000 + millis)
}

fn utc_parts(ms: i64) -> (i64, i64, i64, i64, i64, i64) {
    let secs = ms.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let rem = secs.rem_euclid(86400);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-11-10T20:19:08+00:00`, the form the registry has always stored.
pub fn ms_to_iso(ms: i64) -> String {
    let (y, m, d, h, mi, s) = utc_parts(ms);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00")
}

pub fn now_iso() -> String {
    ms_to_iso(now_ms())
}

/// `2026-11-10` in UTC, for plain command-line output.
pub fn ms_to_date(ms: i64) -> String {
    let (y, m, d, ..) = utc_parts(ms);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `2026-11-10 20:19:08`, for the server log.
pub fn log_stamp() -> String {
    let (y, m, d, h, mi, s) = utc_parts(now_ms());
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}Z")
}

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    PathBuf::from(std::env::var_os(var).unwrap_or_default())
}

/// Shorten a path under the home folder to `~/...`.
pub fn tilde(path: &str) -> String {
    let home = home();
    let home = home.to_string_lossy();
    if home.is_empty() {
        return path.to_string();
    }
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home.as_ref()) {
        Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// Expand a leading `~` to the home folder.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home();
    }
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Collapse every run of whitespace to one space.
pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How long ago, in the short form the command line prints.
pub fn age(ms: Option<i64>) -> String {
    let Some(ms) = ms.filter(|ms| *ms > 0) else { return "-".into() };
    let secs = ((now_ms() - ms) / 1000).max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(iso_to_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_ms("2026-11-10T06:19:56.638Z"), Some(1794291596638));
        assert_eq!(iso_to_ms("2026-11-10T20:19:08+00:00"), Some(1794341948000));
        assert_eq!(iso_to_ms("2026-11-10T11:49:56+05:30"), Some(1794291596000));
        assert_eq!(iso_to_ms("2026-11-10T06:19:56.6Z"), Some(1794291596600));
        assert_eq!(iso_to_ms("not a date"), None);
        assert_eq!(iso_to_ms(""), None);
    }

    #[test]
    fn formats_round_trip() {
        let ms = 1794341948000;
        assert_eq!(ms_to_iso(ms), "2026-11-10T20:19:08+00:00");
        assert_eq!(iso_to_ms(&ms_to_iso(ms)), Some(ms));
        assert_eq!(ms_to_date(ms), "2026-11-10");
        assert_eq!(ms_to_date(951782400000), "2000-02-29");
    }

    #[test]
    fn squashes_whitespace() {
        assert_eq!(squash("  a \n\t b  c "), "a b c");
    }
}
