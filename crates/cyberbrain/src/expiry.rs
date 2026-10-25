//! When a token stops working.
//!
//! One rule for every token this program hands out (MCP clients, hub principals, device
//! tokens): an expiry is optional, a token without one works as it always did, and one past
//! its expiry is refused by name rather than treated as unknown, so whoever holds it learns
//! to ask for a new one instead of hunting for a typo.

use cyberbrain_core::{Error, Result};

/// How long before the end a token counts as "running out", for alerts and lists.
pub const SOON_DAYS: i64 = 7;

/// Read `--expires`: a period from now (`P30D`, `P12W`, `P6M`, `P1Y`), a date (`2027-03-31`,
/// valid through the end of that day, UTC) or an RFC 3339 timestamp. Returns the moment it
/// stops working, as RFC 3339 in UTC. A moment that is not in the future is refused: a token
/// that is dead on arrival is a mistake, not a choice.
pub fn parse(value: &str, now: jiff::Timestamp) -> Result<String> {
    let v = value.trim();
    let bad = |why: String| Error::Config(format!("--expires `{v}`: {why}"));
    let end: jiff::Timestamp = if v.starts_with('P') || v.starts_with('p') {
        let span: jiff::Span = v
            .to_ascii_uppercase()
            .parse()
            .map_err(|e| bad(format!("{e}")))?;
        now.to_zoned(jiff::tz::TimeZone::UTC)
            .checked_add(span)
            .map_err(|e| bad(format!("{e}")))?
            .timestamp()
    } else if let Ok(t) = v.parse::<jiff::Timestamp>() {
        t
    } else if let Ok(d) = v.parse::<jiff::civil::Date>() {
        // Through the end of the day: "expires 31 March" means it still works on the 31st.
        d.at(23, 59, 59, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .map_err(|e| bad(format!("{e}")))?
            .timestamp()
    } else {
        return Err(bad(
            "give a period (P30D, P6M, P1Y), a date (2027-03-31) or a timestamp".into(),
        ));
    };
    if end <= now {
        return Err(bad("that is not in the future".into()));
    }
    jiff::Timestamp::from_second(end.as_second())
        .map(|t| t.to_string())
        .map_err(|e| bad(format!("{e}")))
}

/// Where a token stands.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// No expiry: works until it is revoked, as every token did before expiries existed.
    Open,
    Valid,
    /// Works, and stops within [`SOON_DAYS`].
    Soon,
    Expired,
}

/// Judge an expiry against `now`. An expiry that cannot be read counts as expired: the
/// program wrote it, so an unreadable one is damage, and damage must not keep a door open.
pub fn state(expires_at: Option<&str>, now: jiff::Timestamp) -> State {
    let Some(raw) = expires_at.filter(|s| !s.is_empty()) else {
        return State::Open;
    };
    let Ok(end) = raw.parse::<jiff::Timestamp>() else {
        return State::Expired;
    };
    if end <= now {
        State::Expired
    } else if end.as_second() - now.as_second() <= SOON_DAYS * 86_400 {
        State::Soon
    } else {
        State::Valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> jiff::Timestamp {
        "2026-10-25T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn periods_dates_and_timestamps_are_read() {
        assert_eq!(parse("P30D", now()).unwrap(), "2026-11-24T12:00:00Z");
        assert_eq!(parse("P1Y", now()).unwrap(), "2027-10-25T12:00:00Z");
        assert_eq!(parse("2027-03-31", now()).unwrap(), "2027-03-31T23:59:59Z");
        assert_eq!(
            parse("2027-01-01T08:00:00+01:00", now()).unwrap(),
            "2027-01-01T07:00:00Z"
        );
        for bad in ["2020-01-01", "P0D", "tomorrow", ""] {
            assert!(parse(bad, now()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_token_without_an_expiry_stays_open() {
        assert_eq!(state(None, now()), State::Open);
        assert_eq!(state(Some(""), now()), State::Open);
        assert_eq!(state(Some("2026-10-25T11:59:59Z"), now()), State::Expired);
        assert_eq!(state(Some("2026-10-30T00:00:00Z"), now()), State::Soon);
        assert_eq!(state(Some("2026-12-30T00:00:00Z"), now()), State::Valid);
        assert_eq!(state(Some("garbage"), now()), State::Expired);
    }
}
