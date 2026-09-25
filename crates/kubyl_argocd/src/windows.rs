//! Sync windows of an `AppProject`: whether one is active now, and whether the active ones
//! block syncing an app (Argo CD's rules: an active `deny` window blocks; with `allow` windows,
//! one of them must be active; `manualSync` lets manual syncs through).
//!
//! Schedules are 5-field cron expressions (`0 22 * * *`) with a Go duration (`8h`). A window is
//! active when a scheduled start lies within the last `duration`, however long (Argo CD:
//! `schedule.Next(now - duration) < now`). The search skips days and hours that can't match.

use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _, Zoned};

use crate::model::{Application, SyncWindow};

/// A parsed cron field: allowed values.
#[derive(Clone, Debug, PartialEq)]
struct Field(Vec<bool>);

impl Field {
    fn parse(text: &str, min: u32, max: u32, names: &[&str]) -> Option<Self> {
        let mut allowed = vec![false; max as usize + 1];
        for part in text.split(',') {
            let (range, step) = match part.split_once('/') {
                Some((range, step)) => (range, step.parse::<u32>().ok().filter(|s| *s > 0)?),
                None => (part, 1),
            };
            let value = |s: &str| -> Option<u32> {
                s.parse::<u32>().ok().or_else(|| {
                    names
                        .iter()
                        .position(|n| n.eq_ignore_ascii_case(s))
                        .map(|i| i as u32 + min)
                })
            };
            let (start, end) = if range == "*" || range == "?" {
                (min, max)
            } else if let Some((a, b)) = range.split_once('-') {
                (value(a)?, value(b)?)
            } else {
                let v = value(range)?;
                // `5/15` means from 5 to the end.
                (v, if part.contains('/') { max } else { v })
            };
            if start < min || end > max || start > end {
                return None;
            }
            for v in (start..=end).step_by(step as usize) {
                allowed[v as usize] = true;
            }
        }
        Some(Field(allowed))
    }

    fn has(&self, value: u32) -> bool {
        self.0.get(value as usize).copied().unwrap_or(false)
    }

    fn is_full(&self, min: u32) -> bool {
        self.0.iter().skip(min as usize).all(|v| *v)
    }
}

/// A 5-field cron schedule (minute hour day-of-month month day-of-week), with names and the
/// `@hourly`/`@daily`/… shortcuts.
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    minute: Field,
    hour: Field,
    day: Field,
    month: Field,
    weekday: Field,
}

const MONTHS: &[&str] = &[
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: &[&str] = &["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl Schedule {
    pub fn parse(text: &str) -> Option<Self> {
        let text = match text.trim() {
            "@yearly" | "@annually" => "0 0 1 1 *",
            "@monthly" => "0 0 1 * *",
            "@weekly" => "0 0 * * 0",
            "@daily" | "@midnight" => "0 0 * * *",
            "@hourly" => "0 * * * *",
            other => other,
        };
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return None;
        };
        let mut weekday = Field::parse(weekday, 0, 7, WEEKDAYS)?;
        // 7 is Sunday too.
        if weekday.has(7) {
            weekday.0[0] = true;
        }
        Some(Self {
            minute: Field::parse(minute, 0, 59, &[])?,
            hour: Field::parse(hour, 0, 23, &[])?,
            day: Field::parse(day, 1, 31, &[])?,
            month: Field::parse(month, 1, 12, MONTHS)?,
            weekday,
        })
    }

    /// Whether the schedule fires on this day (month and day fields).
    fn day_matches(&self, time: &Zoned) -> bool {
        let day_of_month = self.day.has(time.day() as u32);
        let day_of_week = self
            .weekday
            .has(time.weekday().to_sunday_zero_offset() as u32);
        // Like cron: when both day fields are restricted, either may match.
        let day = match (self.day.is_full(1), self.weekday.is_full(0)) {
            (true, _) => day_of_week,
            (false, true) => day_of_month,
            (false, false) => day_of_month || day_of_week,
        };
        self.month.has(time.month() as u32) && day
    }
}

/// A Go duration (`1h`, `30m`, `1h30m`, `90s`) in minutes, rounded up.
pub fn parse_duration_minutes(text: &str) -> Option<i64> {
    let mut total_seconds: f64 = 0.0;
    let mut number = String::new();
    let mut chars = text.trim().chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
            continue;
        }
        let mut unit = c.to_string();
        if c == 'm' && chars.peek() == Some(&'s') {
            unit.push(chars.next()?);
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        total_seconds += value
            * match unit.as_str() {
                "h" => 3600.0,
                "m" => 60.0,
                "s" => 1.0,
                "ms" => 0.001,
                _ => return None,
            };
        any = true;
    }
    if !number.is_empty() || !any {
        return None;
    }
    Some((total_seconds / 60.0).ceil() as i64)
}

/// Whether `window` is active at `now`.
pub fn is_active(window: &SyncWindow, now: Timestamp) -> bool {
    let (Some(schedule), Some(minutes)) = (
        Schedule::parse(&window.schedule),
        parse_duration_minutes(&window.duration),
    ) else {
        return false;
    };
    let tz = window
        .time_zone
        .as_deref()
        .and_then(|name| TimeZone::get(name).ok())
        .unwrap_or(TimeZone::UTC);
    let Ok(start) = now
        .to_zoned(tz)
        .with()
        .second(0)
        .subsec_nanosecond(0)
        .build()
    else {
        return false;
    };
    // A start at minute t covers [t, t + duration): look for one in (start - duration, start].
    let Ok(earliest) = start.checked_sub(minutes.minutes()) else {
        return false;
    };
    let mut time = start;
    while time > earliest {
        let previous = if !schedule.day_matches(&time) {
            // The last minute of the day before (DST-safe: from the start of this day).
            time.start_of_day().and_then(|t| t.checked_sub(1.minute()))
        } else if !schedule.hour.has(time.hour() as u32) {
            // The last minute of the hour before.
            time.with()
                .minute(0)
                .build()
                .and_then(|t| t.checked_sub(1.minute()))
        } else if schedule.minute.has(time.minute() as u32) {
            return true;
        } else {
            time.checked_sub(1.minute())
        };
        match previous {
            Ok(previous) if previous < time => time = previous,
            _ => return false,
        }
    }
    false
}

/// A glob as Argo CD matches app names, namespaces and clusters (`*`, `?`, `guestbook-*`).
pub fn glob_match(pattern: &str, value: &str) -> bool {
    fn go(p: &[u8], v: &[u8]) -> bool {
        match (p.first(), v.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], v) || (!v.is_empty() && go(p, &v[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &v[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &v[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), value.as_bytes())
}

/// Whether `window` applies to `app` (by name, destination namespace or cluster).
pub fn applies_to(window: &SyncWindow, app: &Application) -> bool {
    let destination = &app.spec.destination;
    window
        .applications
        .iter()
        .any(|p| glob_match(p, app.name()))
        || destination
            .namespace
            .as_deref()
            .is_some_and(|ns| window.namespaces.iter().any(|p| glob_match(p, ns)))
        || window.clusters.iter().any(|p| {
            destination
                .server
                .as_deref()
                .is_some_and(|s| glob_match(p, s))
                || destination
                    .name
                    .as_deref()
                    .is_some_and(|n| glob_match(p, n))
        })
}

/// What the project's windows say about syncing `app` now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No window applies, or an allow window is active and no deny window is.
    Allowed,
    /// Blocked, except manual syncs (`manualSync: true` on the blocking windows).
    ManualOnly,
    Blocked,
}

/// Argo CD's `CanSync` for the windows that apply to an app.
pub fn verdict(windows: &[SyncWindow], app: &Application, now: Timestamp) -> Verdict {
    let matching: Vec<&SyncWindow> = windows.iter().filter(|w| applies_to(w, app)).collect();
    let active: Vec<&&SyncWindow> = matching.iter().filter(|w| is_active(w, now)).collect();
    let deny: Vec<&&&SyncWindow> = active.iter().filter(|w| w.kind == "deny").collect();
    if !deny.is_empty() {
        return if deny.iter().all(|w| w.manual_sync) {
            Verdict::ManualOnly
        } else {
            Verdict::Blocked
        };
    }
    let allow_windows: Vec<&&SyncWindow> = matching.iter().filter(|w| w.kind == "allow").collect();
    if allow_windows.is_empty() || active.iter().any(|w| w.kind == "allow") {
        return Verdict::Allowed;
    }
    if allow_windows.iter().all(|w| w.manual_sync) {
        Verdict::ManualOnly
    } else {
        Verdict::Blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(kind: &str, schedule: &str, duration: &str) -> SyncWindow {
        SyncWindow {
            kind: kind.into(),
            schedule: schedule.into(),
            duration: duration.into(),
            applications: vec!["*".into()],
            ..Default::default()
        }
    }

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_minutes("8h"), Some(480));
        assert_eq!(parse_duration_minutes("1h30m"), Some(90));
        assert_eq!(parse_duration_minutes("90s"), Some(2));
        assert_eq!(parse_duration_minutes("10"), None);
        assert_eq!(parse_duration_minutes(""), None);
        assert_eq!(parse_duration_minutes("5x"), None);
    }

    #[test]
    fn schedules() {
        assert!(Schedule::parse("0 22 * * *").is_some());
        assert!(Schedule::parse("*/15 9-17 * * mon-fri").is_some());
        assert!(Schedule::parse("@daily").is_some());
        assert!(Schedule::parse("61 * * * *").is_none());
        assert!(Schedule::parse("* * *").is_none());
    }

    #[test]
    fn windows_cover_their_duration() {
        let nightly = window("deny", "0 22 * * *", "8h");
        assert!(is_active(&nightly, at("2026-09-25T23:30:00Z")));
        assert!(is_active(&nightly, at("2026-09-26T05:59:00Z")));
        assert!(!is_active(&nightly, at("2026-09-26T06:00:00Z")));
        assert!(!is_active(&nightly, at("2026-09-25T21:59:00Z")));
        // Weekdays only: Friday 2026-09-25 yes, Saturday no.
        let office = window("allow", "0 9 * * 1-5", "8h");
        assert!(is_active(&office, at("2026-09-25T10:00:00Z")));
        assert!(!is_active(&office, at("2026-09-26T10:00:00Z")));
        // Time zones shift the schedule.
        let berlin = SyncWindow {
            time_zone: Some("Europe/Berlin".into()),
            ..window("deny", "0 22 * * *", "1h")
        };
        assert!(is_active(&berlin, at("2026-09-25T20:30:00Z")));
        assert!(!is_active(&berlin, at("2026-09-25T22:30:00Z")));
        // Longer than a week: a monthly window open for 30 days.
        let monthly = window("allow", "0 0 1 * *", "720h");
        assert!(is_active(&monthly, at("2026-09-20T12:00:00Z")));
        assert!(!is_active(&monthly, at("2026-10-31T12:00:00Z")));
        assert!(!is_active(
            &window("allow", "0 0 1 * *", "24h"),
            at("2026-09-20T12:00:00Z")
        ));
        // Across a DST change (Berlin skips 02:00-03:00 on 2026-03-29).
        let late = SyncWindow {
            time_zone: Some("Europe/Berlin".into()),
            ..window("deny", "30 23 * * *", "5h")
        };
        assert!(is_active(&late, at("2026-03-29T01:30:00Z")));
        assert!(!is_active(&late, at("2026-03-29T03:30:00Z")));
        // Rare schedules with long durations stay cheap: Feb 29 within ten years.
        let leap = window("allow", "0 0 29 2 *", "87600h");
        assert!(is_active(&leap, at("2026-09-25T00:00:00Z")));
    }

    #[test]
    fn verdicts() {
        let app = crate::model::Application::parse(&crate::model::tests::guestbook()).unwrap();
        let night = at("2026-09-25T23:00:00Z");
        let noon = at("2026-09-25T12:00:00Z");
        let deny = window("deny", "0 22 * * *", "8h");
        assert_eq!(
            verdict(std::slice::from_ref(&deny), &app, noon),
            Verdict::Allowed
        );
        assert_eq!(
            verdict(std::slice::from_ref(&deny), &app, night),
            Verdict::Blocked
        );
        let manual = SyncWindow {
            manual_sync: true,
            ..deny.clone()
        };
        assert_eq!(verdict(&[manual], &app, night), Verdict::ManualOnly);
        // An allow window that isn't active blocks.
        let allow = window("allow", "0 9 * * *", "2h");
        assert_eq!(
            verdict(std::slice::from_ref(&allow), &app, noon),
            Verdict::Blocked
        );
        assert_eq!(
            verdict(&[allow], &app, at("2026-09-25T10:00:00Z")),
            Verdict::Allowed
        );
        // Windows for other apps don't count.
        let other = SyncWindow {
            applications: vec!["payments-*".into()],
            ..window("deny", "* * * * *", "1h")
        };
        assert_eq!(verdict(&[other], &app, noon), Verdict::Allowed);
        // Matching by destination namespace.
        let by_namespace = SyncWindow {
            applications: vec![],
            namespaces: vec!["guest*".into()],
            ..window("deny", "* * * * *", "1h")
        };
        assert_eq!(verdict(&[by_namespace], &app, noon), Verdict::Blocked);
    }

    #[test]
    fn globs() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("guestbook-*", "guestbook-dev"));
        assert!(!glob_match("guestbook-*", "guestbook"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
    }
}
