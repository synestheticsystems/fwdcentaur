//! The mechanics of the calendar, for whatever gives a day a meaning: a `YYYY-MM-DD` day as a number
//! and back, and how far this machine's clock is from UTC. What a typed day means belongs to its
//! user: noon UTC in the decision log (`decision_log::parse_date`), and the start of that day where
//! the command runs for a task's not-before date (`not_before::from_day`).

use anyhow::{bail, Result};

pub(crate) const DAY_MS: i64 = 86_400_000;

/// The calendar day of `ms` in a time zone `offset_ms` east of UTC.
pub fn date_at(ms: i64, offset_ms: i64) -> String {
    let (y, m, d) = civil_from_days((ms + offset_ms).div_euclid(DAY_MS));
    format!("{y:04}-{m:02}-{d:02}")
}

/// How far east of UTC this machine's clock is at `ms`, daylight saving included.
#[cfg(all(feature = "native", unix))]
pub(crate) fn local_offset_ms(ms: i64) -> i64 {
    let secs = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` it is given, which is zeroed plain data.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let found = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if found {
        // `c_long` is 64 bits wide on the targets fwd builds for; the cast keeps the rest honest.
        #[allow(clippy::unnecessary_cast)]
        let seconds_east = tm.tm_gmtoff as i64;
        seconds_east * 1000
    } else {
        0
    }
}

/// How far east of UTC the browser's clock is at `ms`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn local_offset_ms(ms: i64) -> i64 {
    -(js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64)).get_timezone_offset() as i64) * 60_000
}

/// Elsewhere (the page's native tests), UTC.
#[cfg(not(any(all(feature = "native", unix), target_arch = "wasm32")))]
pub(crate) fn local_offset_ms(_ms: i64) -> i64 {
    0
}

/// The number since 1970-01-01 of a `YYYY-MM-DD` day.
pub(crate) fn parse_day(s: &str) -> Result<i64> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    let parsed = match parts.as_slice() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => {
            (y.parse::<i64>().ok(), m.parse::<u32>().ok(), d.parse::<u32>().ok())
        }
        _ => (None, None, None),
    };
    let (Some(y), Some(m), Some(d)) = parsed else {
        bail!("`{s}` is not a day: write it as YYYY-MM-DD");
    };
    let days = days_from_civil(y, m, d);
    if !(1..=12).contains(&m) || civil_from_days(days) != (y, m, d) {
        bail!("`{s}` is not a day of the calendar");
    }
    Ok(days)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_is_its_number_since_the_epoch() {
        assert_eq!(parse_day("1970-01-01").unwrap(), 0);
        assert_eq!(parse_day("1970-01-02").unwrap(), 1);
        assert_eq!(parse_day("1969-12-31").unwrap(), -1);
        assert_eq!(parse_day("2000-03-01").unwrap() - parse_day("2000-02-28").unwrap(), 2, "a leap year's February");
        assert_eq!(parse_day(" 2026-09-24 ").unwrap(), parse_day("2026-09-24").unwrap(), "space around it");
    }

    #[test]
    fn what_is_no_day_is_refused_saying_why() {
        for wrong_form in ["2026-9-24", "24-09-2026", "yesterday", ""] {
            let refusal = parse_day(wrong_form).unwrap_err().to_string();
            assert_eq!(refusal, format!("`{wrong_form}` is not a day: write it as YYYY-MM-DD"));
        }
        for no_such_day in ["2026-02-30", "2026-02-29", "2026-13-01", "2026-00-10"] {
            let refusal = parse_day(no_such_day).unwrap_err().to_string();
            assert_eq!(refusal, format!("`{no_such_day}` is not a day of the calendar"));
        }
    }

    #[test]
    fn an_instant_falls_on_the_day_of_its_time_zone() {
        const HOUR: i64 = 3_600_000;
        assert_eq!(date_at(0, 0), "1970-01-01");
        assert_eq!(date_at(-1, 0), "1969-12-31", "before the epoch");
        let midnight_utc = parse_day("2000-02-29").unwrap() * DAY_MS;
        assert_eq!(date_at(midnight_utc, 0), "2000-02-29");
        assert_eq!(date_at(midnight_utc, -HOUR), "2000-02-28", "an hour west");
        assert_eq!(date_at(midnight_utc - 1, HOUR), "2000-02-29", "a moment earlier, an hour east");
    }
}
