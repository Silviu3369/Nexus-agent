//! When an automation runs next, in this PC's local time (or a given time
//! zone, for tests), through Windows' own time-zone rules: when the clock
//! changes (summer/winter time) the moment in UTC moves and the local hour
//! stays. A local time that does not exist that day (the hour skipped in
//! spring) or exists twice (the hour repeated in autumn) is resolved by
//! Windows' conversion.

use nexus_protocol::Schedule;
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    TIME_ZONE_INFORMATION, TzSpecificLocalTimeToSystemTime,
};

/// From 1601-01-01 (FILETIME) to 1970-01-01, in 100 ns.
const EPOCH_100NS: i64 = 116_444_736_000_000_000;
const DAY_MS: i64 = 86_400_000;

fn zero() -> SYSTEMTIME {
    SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    }
}

fn ms_to_systemtime(ms: i64) -> Option<SYSTEMTIME> {
    let ticks = ms.checked_mul(10_000)?.checked_add(EPOCH_100NS)?;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut st = zero();
    // SAFETY: valid input and output structures.
    (unsafe { FileTimeToSystemTime(&ft, &mut st) } != 0).then_some(st)
}

fn systemtime_to_ms(st: &SYSTEMTIME) -> Option<i64> {
    let mut ft = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: valid input and output structures.
    if unsafe { SystemTimeToFileTime(st, &mut ft) } == 0 {
        return None;
    }
    let ticks = ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64;
    Some((ticks - EPOCH_100NS) / 10_000)
}

fn zone_ptr(tz: Option<&TIME_ZONE_INFORMATION>) -> *const TIME_ZONE_INFORMATION {
    tz.map_or(std::ptr::null(), |z| z as *const _)
}

/// The local wall clock at a UTC moment (`None` zone: this PC's).
fn to_local(ms: i64, tz: Option<&TIME_ZONE_INFORMATION>) -> Option<SYSTEMTIME> {
    let utc = ms_to_systemtime(ms)?;
    let mut local = zero();
    // SAFETY: valid structures; a null zone means the current one.
    (unsafe { SystemTimeToTzSpecificLocalTime(zone_ptr(tz), &utc, &mut local) } != 0)
        .then_some(local)
}

/// The UTC moment of a local wall-clock time.
fn from_local(local: &SYSTEMTIME, tz: Option<&TIME_ZONE_INFORMATION>) -> Option<i64> {
    let mut utc = zero();
    // SAFETY: valid structures; a null zone means the current one.
    if unsafe { TzSpecificLocalTimeToSystemTime(zone_ptr(tz), local, &mut utc) } == 0 {
        return None;
    }
    systemtime_to_ms(&utc)
}

/// Whether a schedule can run at all.
pub fn validate(schedule: &Schedule) -> Result<(), String> {
    match schedule {
        Schedule::Once { at_ms } => {
            if *at_ms <= 0 {
                return Err("choose a date and time".to_string());
            }
        }
        Schedule::Weekly { days, hour, minute } => {
            if days.is_empty() {
                return Err("choose at least one day".to_string());
            }
            if days.iter().any(|d| *d > 6) {
                return Err("days are 0 (Monday) to 6 (Sunday)".to_string());
            }
            if *hour > 23 || *minute > 59 {
                return Err("the time must be 00:00 to 23:59".to_string());
            }
        }
    }
    Ok(())
}

/// The next run strictly after `after_ms`; `None` once a one-time schedule
/// has passed.
pub fn next_run(
    schedule: &Schedule,
    after_ms: i64,
    tz: Option<&TIME_ZONE_INFORMATION>,
) -> Option<i64> {
    match schedule {
        Schedule::Once { at_ms } => (*at_ms > after_ms).then_some(*at_ms),
        Schedule::Weekly { days, hour, minute } => {
            let mut day = to_local(after_ms, tz)?;
            day.wHour = 0;
            day.wMinute = 0;
            day.wSecond = 0;
            day.wMilliseconds = 0;
            // The local date as a plain calendar day, for day arithmetic.
            let first = systemtime_to_ms(&day)?;
            for d in 0..=7 {
                let date = ms_to_systemtime(first + d * DAY_MS)?;
                // Windows counts from Sunday; schedules from Monday.
                let weekday = ((date.wDayOfWeek + 6) % 7) as u8;
                if !days.contains(&weekday) {
                    continue;
                }
                let mut at = date;
                at.wHour = u16::from(*hour);
                at.wMinute = u16::from(*minute);
                let utc = from_local(&at, tz)?;
                if utc > after_ms {
                    return Some(utc);
                }
            }
            None
        }
    }
}

/// "2026-10-18": the local day at a moment (`None` zone: this PC's).
pub fn local_date(ms: i64, tz: Option<&TIME_ZONE_INFORMATION>) -> String {
    match to_local(ms, tz) {
        Some(t) => format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay),
        None => String::new(),
    }
}

/// "2026-10-18 09:00" in local time, for a run's task title.
pub fn local_label(ms: i64, tz: Option<&TIME_ZONE_INFORMATION>) -> String {
    match to_local(ms, tz) {
        Some(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute
        ),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Central European time (Romance Standard Time): UTC+1, summer time from the last
    /// Sunday of March 02:00 to the last Sunday of October 03:00.
    fn central_europe() -> TIME_ZONE_INFORMATION {
        let rule = |month: u16, hour: u16| SYSTEMTIME {
            wMonth: month,
            wDayOfWeek: 0,
            wDay: 5,
            wHour: hour,
            ..zero()
        };
        // SAFETY: zeroed POD is a valid starting value.
        let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
        tz.Bias = -60;
        tz.StandardDate = rule(10, 3);
        tz.StandardBias = 0;
        tz.DaylightDate = rule(3, 2);
        tz.DaylightBias = -60;
        tz
    }

    fn utc(y: u16, m: u16, d: u16, h: u16, mi: u16) -> i64 {
        systemtime_to_ms(&SYSTEMTIME {
            wYear: y,
            wMonth: m,
            wDay: d,
            wHour: h,
            wMinute: mi,
            ..zero()
        })
        .unwrap()
    }

    const EVERY_DAY: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

    #[test]
    fn a_daily_time_keeps_its_local_hour_across_the_clock_change() {
        let tz = central_europe();
        let nine = Schedule::Weekly {
            days: EVERY_DAY.to_vec(),
            hour: 9,
            minute: 0,
        };
        // Saturday 24 October 2026, still summer time (UTC+2).
        assert_eq!(
            next_run(&nine, utc(2026, 10, 23, 12, 0), Some(&tz)),
            Some(utc(2026, 10, 24, 7, 0))
        );
        // Sunday 25 October: the clock went back at 03:00, so 09:00 is UTC+1.
        assert_eq!(
            next_run(&nine, utc(2026, 10, 24, 12, 0), Some(&tz)),
            Some(utc(2026, 10, 25, 8, 0))
        );
        // And back to summer time on 29 March 2027.
        assert_eq!(
            next_run(&nine, utc(2027, 3, 28, 12, 0), Some(&tz)),
            Some(utc(2027, 3, 29, 7, 0))
        );
        assert_eq!(
            local_label(utc(2026, 10, 25, 8, 0), Some(&tz)),
            "2026-10-25 09:00"
        );
    }

    #[test]
    fn a_time_inside_the_changed_hour_runs_once_that_day() {
        let tz = central_europe();
        let half_past_two = Schedule::Weekly {
            days: EVERY_DAY.to_vec(),
            hour: 2,
            minute: 30,
        };
        // 25 October 2026: 02:30 happens twice (03:00 goes back to 02:00).
        let repeated = next_run(&half_past_two, utc(2026, 10, 24, 12, 0), Some(&tz)).unwrap();
        assert!((utc(2026, 10, 25, 0, 30)..=utc(2026, 10, 25, 1, 30)).contains(&repeated));
        assert_eq!(
            next_run(&half_past_two, repeated, Some(&tz)),
            Some(utc(2026, 10, 26, 1, 30))
        );
        // 28 March 2027: 02:30 does not exist (02:00 jumps to 03:00).
        let skipped = next_run(&half_past_two, utc(2027, 3, 27, 12, 0), Some(&tz)).unwrap();
        assert!((utc(2027, 3, 28, 0, 0)..=utc(2027, 3, 28, 2, 0)).contains(&skipped));
        assert_eq!(
            next_run(&half_past_two, skipped, Some(&tz)),
            Some(utc(2027, 3, 29, 0, 30))
        );
    }

    #[test]
    fn chosen_days_skip_the_others_and_a_time_already_past_moves_on() {
        let tz = central_europe();
        let workdays = Schedule::Weekly {
            days: vec![0, 1, 2, 3, 4],
            hour: 9,
            minute: 30,
        };
        // Friday 23 October 2026 at 12:00 local: next is Monday 26th, 09:30 (UTC+1).
        assert_eq!(
            next_run(&workdays, utc(2026, 10, 23, 10, 0), Some(&tz)),
            Some(utc(2026, 10, 26, 8, 30))
        );
        // Friday 08:00 local: still today at 09:30 (UTC+2).
        assert_eq!(
            next_run(&workdays, utc(2026, 10, 23, 6, 0), Some(&tz)),
            Some(utc(2026, 10, 23, 7, 30))
        );
        // Exactly at the time: the next one, not the same one again.
        assert_eq!(
            next_run(&workdays, utc(2026, 10, 23, 7, 30), Some(&tz)),
            Some(utc(2026, 10, 26, 8, 30))
        );
    }

    #[test]
    fn a_one_time_schedule_runs_once() {
        let at = utc(2026, 11, 1, 18, 0);
        let once = Schedule::Once { at_ms: at };
        assert_eq!(next_run(&once, at - 1, None), Some(at));
        assert_eq!(next_run(&once, at, None), None);
    }

    #[test]
    fn schedules_that_cannot_run_are_refused() {
        let weekly = |days: Vec<u8>, hour, minute| Schedule::Weekly { days, hour, minute };
        assert!(validate(&weekly(vec![], 9, 0)).is_err());
        assert!(validate(&weekly(vec![7], 9, 0)).is_err());
        assert!(validate(&weekly(vec![0], 24, 0)).is_err());
        assert!(validate(&weekly(vec![0], 9, 60)).is_err());
        assert!(validate(&Schedule::Once { at_ms: 0 }).is_err());
        assert!(validate(&weekly(vec![0, 6], 23, 59)).is_ok());
    }

    #[test]
    fn this_pcs_zone_gives_a_moment_within_a_week() {
        let now = nexus_storage::now_ms();
        let daily = Schedule::Weekly {
            days: EVERY_DAY.to_vec(),
            hour: 6,
            minute: 15,
        };
        let next = next_run(&daily, now, None).unwrap();
        assert!(next > now && next - now <= DAY_MS + 3_600_000);
    }
}
