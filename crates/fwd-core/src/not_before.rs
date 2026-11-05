//! A task's not-before date as people type it and read it. The task stores an instant, so that every
//! machine agrees on when it comes due (`rules::not_yet_due` says what the date holds back; the
//! date is `model::Task::not_before`), while people think in days on their own calendar: a day typed
//! means its start in the time zone of the machine running the command, and a date reads back in
//! the reader's.

use crate::calendar::{date_at, local_offset_ms, parse_day, DAY_MS};
use anyhow::Result;

/// The instant a `YYYY-MM-DD` day starts in this machine's time zone.
pub fn from_day(day: &str) -> Result<i64> {
    Ok(start_of_day(parse_day(day)?, local_offset_ms))
}

/// The first instant of the day numbered `days`, on a clock `offset_at(ms)` east of UTC at `ms`.
/// A change of the clocks can lie between that day's midnights in UTC and on the clock, so this
/// tries the offsets from before and after any change and takes the earlier instant that falls on
/// the day: where clocks go forward at midnight the day starts when they land, and where they go
/// back across midnight it starts at the first midnight. A day the clocks skip whole starts when
/// they land.
fn start_of_day(days: i64, offset_at: impl Fn(i64) -> i64) -> i64 {
    let midnight_utc = days * DAY_MS;
    let starts = [midnight_utc - DAY_MS, midnight_utc + DAY_MS].map(|ms| midnight_utc - offset_at(ms));
    let on_the_day = |at: &i64| (at + offset_at(*at)).div_euclid(DAY_MS) == days;
    starts.into_iter().filter(on_the_day).min().unwrap_or(starts[0])
}

/// What a date says to whoever reads it, in this machine's time zone (the browser's, in the page):
/// `not before 2026-12-01` at the very start of that day, which is how a day typed without a time
/// reads back where it was typed, and `not before 2026-12-01 09:30` at any other time, so that a
/// date typed elsewhere says the hour it comes here.
pub fn words(ms: i64) -> String {
    words_at(ms, local_offset_ms(ms))
}

/// [`words`] in a time zone `offset_ms` east of UTC.
fn words_at(ms: i64, offset_ms: i64) -> String {
    let day = date_at(ms, offset_ms);
    match (ms + offset_ms).rem_euclid(DAY_MS) {
        0 => format!("not before {day}"),
        since_midnight => {
            let minutes = since_midnight / 60_000;
            format!("not before {day} {:02}:{:02}", minutes / 60, minutes % 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;

    fn day(s: &str) -> i64 {
        parse_day(s).unwrap()
    }

    /// `hour` o'clock UTC on the day `s`.
    fn utc(s: &str, hour: i64) -> i64 {
        day(s) * DAY_MS + hour * HOUR
    }

    #[test]
    fn a_day_starts_at_midnight_on_a_clock_that_does_not_change() {
        assert_eq!(start_of_day(day("2026-12-01"), |_| -8 * HOUR), utc("2026-12-01", 8), "west of UTC");
        assert_eq!(start_of_day(day("2026-12-01"), |_| 10 * HOUR), utc("2026-11-30", 14), "east of UTC");
        assert_eq!(start_of_day(day("2026-12-01"), |_| 0), utc("2026-12-01", 0), "at UTC");
    }

    /// Clocks ten hours east that go forward at 02:00 on the 4th: UTC's midnight that day comes after
    /// the change, and the clock's midnight before it.
    #[test]
    fn a_day_starts_by_the_offset_of_its_own_midnight_when_the_clocks_change_that_morning() {
        let forward = |ms: i64| if ms < utc("2026-10-03", 16) { 10 * HOUR } else { 11 * HOUR };
        assert_eq!(start_of_day(day("2026-10-04"), forward), utc("2026-10-03", 14));
        assert_eq!(start_of_day(day("2026-10-05"), forward), utc("2026-10-04", 13));
    }

    #[test]
    fn a_day_with_no_midnight_starts_when_the_clocks_land() {
        let skips_midnight = |ms: i64| if ms < utc("2026-03-08", 5) { -5 * HOUR } else { -4 * HOUR };
        assert_eq!(start_of_day(day("2026-03-08"), skips_midnight), utc("2026-03-08", 5));
    }

    #[test]
    fn a_day_with_two_midnights_starts_at_the_first() {
        let repeats_midnight = |ms: i64| if ms < utc("2026-11-01", 5) { -4 * HOUR } else { -5 * HOUR };
        assert_eq!(start_of_day(day("2026-11-01"), repeats_midnight), utc("2026-11-01", 4));
    }

    #[test]
    fn a_day_the_clocks_skip_whole_starts_when_they_land() {
        let skips_the_day = |ms: i64| if ms < utc("2011-12-30", 10) { -10 * HOUR } else { 14 * HOUR };
        assert_eq!(start_of_day(day("2011-12-30"), skips_the_day), utc("2011-12-30", 10));
    }

    #[test]
    fn a_day_typed_on_this_machine_starts_where_the_day_before_ends() {
        let start = from_day("2026-12-01").unwrap();
        let date = |ms| date_at(ms, local_offset_ms(ms));
        assert_eq!(date(start), "2026-12-01");
        assert_eq!(date(start - 1), "2026-11-30");
        assert_eq!(words(start), "not before 2026-12-01", "it reads back as it was typed");
        for not_a_day in ["2026-02-30", "2026-13-01", "2026-9-24", "soon", ""] {
            assert!(from_day(not_a_day).is_err(), "{not_a_day}");
        }
    }

    #[test]
    fn a_date_gives_the_time_unless_it_is_the_start_of_the_day_where_it_is_read() {
        let start_in_the_west = utc("2026-12-01", 8);
        assert_eq!(words_at(start_in_the_west, -8 * HOUR), "not before 2026-12-01");
        assert_eq!(words_at(start_in_the_west, 0), "not before 2026-12-01 08:00", "the same instant, read at UTC");
        assert_eq!(words_at(start_in_the_west, -11 * HOUR), "not before 2026-11-30 21:00", "and further west");
        assert_eq!(
            words_at(start_in_the_west + 90 * 60_000, -8 * HOUR),
            "not before 2026-12-01 01:30",
            "later that day"
        );
    }
}
