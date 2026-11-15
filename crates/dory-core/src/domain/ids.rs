use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use super::{InvalidInput, Timestamp};

const MAX_ID_LEN: usize = 128;

/// A recording's identity. It is a path component, so parsing keeps it to 1..=128 characters of
/// `[A-Za-z0-9._-]` that do not start with `.`. Legacy ids (old file stems) satisfy this.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordingId(Arc<str>);

impl RecordingId {
    pub fn parse(s: &str) -> Result<Self, InvalidInput> {
        let bad = |detail: &str| {
            Err(InvalidInput::new(
                "recording id",
                format!("{detail}: {s:?}"),
            ))
        };
        if s.is_empty() {
            return bad("empty");
        }
        if s.len() > MAX_ID_LEN {
            return bad("longer than 128 characters");
        }
        if s.starts_with('.') {
            return bad("starts with a dot");
        }
        if !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return bad("characters outside A-Z a-z 0-9 . - _");
        }
        Ok(Self(s.into()))
    }

    /// A time-ordered id such as `2026-11-15T153012-7f3a`, in UTC. `suffix` keeps two recordings
    /// started in the same second apart.
    pub fn generate(now: Timestamp, suffix: u16) -> Self {
        let secs = now.0.div_euclid(1000);
        let days = secs.div_euclid(86_400);
        let in_day = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        let (hh, mm, ss) = (in_day / 3600, in_day % 3600 / 60, in_day % 60);
        Self(format!("{y:04}-{m:02}-{d:02}T{hh:02}{mm:02}{ss:02}-{suffix:04x}").into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for RecordingId {
    type Err = InvalidInput;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for RecordingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

/// Version of a recording's transcript. Starts at zero (none) and grows by one per committed
/// transcript, so a summary or a set of speaker names can say which transcript it belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision(pub u32);

impl Revision {
    pub fn next(self) -> Revision {
        Revision(self.0.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_table() {
        let long_ok = "a".repeat(128);
        let long_bad = "a".repeat(129);
        let cases: &[(&str, bool)] = &[
            ("2026-11-15T153012-7f3a", true),
            ("1700000000000-k3j2h1", true),
            ("legacy_stem.v2", true),
            ("trailing.", true),
            (long_ok.as_str(), true),
            (long_bad.as_str(), false),
            ("", false),
            (".hidden", false),
            ("..", false),
            ("a/b", false),
            ("a\\b", false),
            ("has space", false),
            ("naïve", false),
            ("nul\0byte", false),
        ];
        for (input, ok) in cases {
            assert_eq!(RecordingId::parse(input).is_ok(), *ok, "{input:?}");
        }
    }

    #[test]
    fn parsed_id_round_trips_through_display() {
        let id: RecordingId = "abc-1.2_3".parse().unwrap();
        assert_eq!(id.to_string(), "abc-1.2_3");
    }

    #[test]
    fn generated_ids_are_utc_sortable_and_parse() {
        let cases = [
            (0i64, 0u16, "1970-01-01T000000-0000"),
            (1_700_000_000_000, 0x7f3a, "2023-11-14T221320-7f3a"),
            (1_709_164_799_999, 1, "2024-02-28T235959-0001"),
            (1_709_164_800_000, 1, "2024-02-29T000000-0001"),
        ];
        for (ms, suffix, want) in cases {
            let id = RecordingId::generate(Timestamp(ms), suffix);
            assert_eq!(id.as_str(), want);
            assert!(RecordingId::parse(id.as_str()).is_ok());
        }
        let earlier = RecordingId::generate(Timestamp(1_000_000), 0xffff);
        let later = RecordingId::generate(Timestamp(2_000_000_000_000), 0);
        assert!(earlier < later);
    }

    #[test]
    fn revision_counts_up() {
        assert_eq!(Revision::default().next(), Revision(1));
        assert!(Revision(2) > Revision(1));
    }
}
