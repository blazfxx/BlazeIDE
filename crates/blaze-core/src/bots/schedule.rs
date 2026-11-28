//! When a routine runs: daily / weekdays / weekly at a time, every N hours,
//! or a 5-field cron line. Times are in the computer's own time zone.

use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Schedule {
    Daily { time: String },
    Weekdays { time: String },
    /// days: 0 = Sunday ... 6 = Saturday.
    Weekly { days: Vec<u8>, time: String },
    Every { hours: u32 },
    Cron { expr: String },
}

fn hm(t: &str) -> Result<(i8, i8)> {
    let (h, m) = t.trim().split_once(':').ok_or_else(|| Error::msg("Use a time like 09:30"))?;
    let h: i8 = h.parse().map_err(|_| Error::msg("Use a time like 09:30"))?;
    let m: i8 = m.parse().map_err(|_| Error::msg("Use a time like 09:30"))?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return Err(Error::msg("Use a time like 09:30"));
    }
    Ok((h, m))
}

/// One cron field as a set of allowed values.
fn field(spec: &str, lo: u32, hi: u32) -> Result<Vec<bool>> {
    let mut ok = vec![false; (hi + 1) as usize];
    for part in spec.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().map_err(|_| Error::msg("Bad cron step"))?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(Error::msg("Bad cron step"));
        }
        let (a, b) = if range == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range.split_once('-') {
            (a.parse().map_err(|_| Error::msg("Bad cron range"))?, b.parse().map_err(|_| Error::msg("Bad cron range"))?)
        } else {
            let v: u32 = range.parse().map_err(|_| Error::msg(format!("Bad cron value {range}")))?;
            (v, if part.contains('/') { hi } else { v })
        };
        if a < lo || b > hi || a > b {
            return Err(Error::msg(format!("Cron value out of range: {part}")));
        }
        let mut v = a;
        while v <= b {
            ok[v as usize] = true;
            v += step;
        }
    }
    Ok(ok)
}

pub struct Cron {
    min: Vec<bool>,
    hour: Vec<bool>,
    dom: Vec<bool>,
    month: Vec<bool>,
    dow: Vec<bool>,
    dom_any: bool,
    dow_any: bool,
}

impl Cron {
    pub fn parse(expr: &str) -> Result<Self> {
        let f: Vec<&str> = expr.split_whitespace().collect();
        if f.len() != 5 {
            return Err(Error::msg("A cron line has 5 fields: minute hour day month weekday"));
        }
        let mut dow = field(&f[4].replace('7', "0"), 0, 6)?;
        if f[4].contains('7') && !f[4].contains('-') {
            dow[0] = true;
        }
        Ok(Cron { min: field(f[0], 0, 59)?, hour: field(f[1], 0, 23)?, dom: field(f[2], 1, 31)?, month: field(f[3], 1, 12)?, dow, dom_any: f[2] == "*", dow_any: f[4] == "*" })
    }

    fn matches(&self, z: &Zoned) -> bool {
        let wd = z.weekday().to_sunday_zero_offset() as usize;
        let day_ok = match (self.dom_any, self.dow_any) {
            (true, true) => true,
            (false, true) => self.dom[z.day() as usize],
            (true, false) => self.dow[wd],
            // Standard cron: either restricted field may match.
            (false, false) => self.dom[z.day() as usize] || self.dow[wd],
        };
        self.min[z.minute() as usize] && self.hour[z.hour() as usize] && self.month[z.month() as usize] && day_ok
    }
}

impl Schedule {
    pub fn validate(&self) -> Result<()> {
        match self {
            Schedule::Daily { time } | Schedule::Weekdays { time } => hm(time).map(|_| ()),
            Schedule::Weekly { days, time } => {
                if days.is_empty() || days.iter().any(|d| *d > 6) {
                    return Err(Error::msg("Pick at least one day"));
                }
                hm(time).map(|_| ())
            }
            Schedule::Every { hours } => {
                if *hours == 0 || *hours > 24 * 31 {
                    Err(Error::msg("Every 1 to 744 hours"))
                } else {
                    Ok(())
                }
            }
            Schedule::Cron { expr } => Cron::parse(expr).map(|_| ()),
        }
    }

    /// The first run strictly after `after`.
    pub fn next_after(&self, after: &Zoned) -> Option<Zoned> {
        let start = after.checked_add(1.minute()).ok()?.with().second(0).subsec_nanosecond(0).build().ok()?;
        match self {
            Schedule::Every { hours } => after.checked_add((*hours as i64).hours()).ok(),
            Schedule::Cron { expr } => {
                let c = Cron::parse(expr).ok()?;
                let mut z = start;
                // Up to a year and a bit of minutes.
                for _ in 0..(370 * 24 * 60) {
                    if c.matches(&z) {
                        return Some(z);
                    }
                    z = z.checked_add(1.minute()).ok()?;
                }
                None
            }
            Schedule::Daily { time } | Schedule::Weekdays { time } | Schedule::Weekly { time, .. } => {
                let (h, m) = hm(time).ok()?;
                for d in 0..9i64 {
                    let day = after.checked_add(d.days()).ok()?;
                    let cand = day.with().hour(h).minute(m).second(0).subsec_nanosecond(0).build().ok()?;
                    if cand <= *after {
                        continue;
                    }
                    let wd = cand.weekday().to_sunday_zero_offset() as u8;
                    let ok = match self {
                        Schedule::Weekdays { .. } => (1..=5).contains(&wd),
                        Schedule::Weekly { days, .. } => days.contains(&wd),
                        _ => true,
                    };
                    if ok {
                        return Some(cand);
                    }
                }
                None
            }
        }
    }

    /// Short text for the UI ("Weekdays at 09:00").
    pub fn describe(&self) -> String {
        match self {
            Schedule::Daily { time } => format!("Every day at {time}"),
            Schedule::Weekdays { time } => format!("Weekdays at {time}"),
            Schedule::Weekly { days, time } => {
                const N: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                format!("{} at {time}", days.iter().filter_map(|d| N.get(*d as usize)).copied().collect::<Vec<_>>().join(", "))
            }
            Schedule::Every { hours } => format!("Every {hours} h"),
            Schedule::Cron { expr } => format!("cron {expr}"),
        }
    }
}

pub fn zoned(unix: i64) -> Option<Zoned> {
    Some(Timestamp::from_second(unix).ok()?.to_zoned(TimeZone::system()))
}

/// Is a routine with this schedule due at `now`, given when it last ran (0 =
/// never) and when it was created? `catch_up` = run a time that passed while
/// nothing was running; otherwise a missed time is skipped and the next one
/// waited for.
pub fn is_due(s: &Schedule, last_run: i64, created: i64, now: i64, catch_up: bool) -> bool {
    let base = if last_run > 0 { last_run } else { created.max(0) };
    let (Some(base_z), Some(now_z)) = (zoned(base), zoned(now)) else { return false };
    let Some(next) = s.next_after(&base_z) else { return false };
    if next > now_z {
        return false;
    }
    if catch_up {
        return true;
    }
    // Without catch-up only run if the due time is recent (this tick or the
    // last few minutes), not hours ago.
    now - next.timestamp().as_second() <= 180
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn z(y: i16, mo: i8, d: i8, h: i8, mi: i8) -> Zoned {
        // Local time, like is_due (it uses the system zone): UTC here only
        // passed on UTC machines.
        date(y, mo, d).at(h, mi, 0, 0).to_zoned(TimeZone::system()).unwrap()
    }

    #[test]
    fn simple_schedules() {
        // 2026-11-24 is a Tuesday.
        let tue_10 = z(2026, 11, 24, 10, 0);
        let daily = Schedule::Daily { time: "09:30".into() };
        assert_eq!(daily.next_after(&tue_10).unwrap(), z(2026, 11, 25, 9, 30));
        assert_eq!(daily.next_after(&z(2026, 11, 24, 9, 0)).unwrap(), z(2026, 11, 24, 9, 30));
        let wk = Schedule::Weekdays { time: "08:00".into() };
        // Friday 9:00 -> Monday 8:00
        assert_eq!(wk.next_after(&z(2026, 11, 27, 9, 0)).unwrap(), z(2026, 11, 30, 8, 0));
        let weekly = Schedule::Weekly { days: vec![0, 3], time: "18:00".into() };
        assert_eq!(weekly.next_after(&tue_10).unwrap(), z(2026, 11, 25, 18, 0));
        let every = Schedule::Every { hours: 6 };
        assert_eq!(every.next_after(&tue_10).unwrap(), z(2026, 11, 24, 16, 0));
        assert!(Schedule::Daily { time: "25:00".into() }.validate().is_err());
        assert!(Schedule::Weekly { days: vec![], time: "10:00".into() }.validate().is_err());
    }

    #[test]
    fn cron_lines() {
        let tue_10 = z(2026, 11, 24, 10, 0);
        let c = |e: &str| Schedule::Cron { expr: e.into() };
        assert_eq!(c("*/15 * * * *").next_after(&tue_10).unwrap(), z(2026, 11, 24, 10, 15));
        assert_eq!(c("0 9 1 * *").next_after(&tue_10).unwrap(), z(2026, 12, 1, 9, 0));
        assert_eq!(c("30 7 * * 1-5").next_after(&z(2026, 11, 28, 8, 0)).unwrap(), z(2026, 11, 30, 7, 30));
        assert_eq!(c("0 12 * * 7").next_after(&tue_10).unwrap(), z(2026, 11, 29, 12, 0), "7 = Sunday");
        assert!(c("61 * * * *").validate().is_err());
        assert!(c("* * *").validate().is_err());
        assert!(c("*/0 * * * *").validate().is_err());
    }

    #[test]
    fn due_rules() {
        let now = z(2026, 11, 24, 9, 31).timestamp().as_second();
        let created = z(2026, 11, 20, 0, 0).timestamp().as_second();
        let s = Schedule::Cron { expr: "30 9 * * *".into() };
        let yesterday = z(2026, 11, 23, 9, 30).timestamp().as_second();
        assert!(is_due(&s, yesterday, created, now, false), "due one minute ago");
        let today = z(2026, 11, 24, 9, 30).timestamp().as_second();
        assert!(!is_due(&s, today, created, now, false), "already ran");
        let long_ago = z(2026, 11, 21, 9, 30).timestamp().as_second();
        let later = z(2026, 11, 24, 15, 0).timestamp().as_second();
        assert!(!is_due(&s, long_ago, created, later, false), "missed and no catch-up");
        assert!(is_due(&s, long_ago, created, later, true), "missed, catch up once");
    }
}
