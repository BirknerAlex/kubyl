//! Time ranges of charts (15m/1h/6h/24h/7d): windows, steps and refresh rates.

use std::time::Duration;

/// How far back a chart looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeRange {
    M15,
    #[default]
    H1,
    H6,
    H24,
    D7,
}

impl TimeRange {
    pub const ALL: [TimeRange; 5] = [
        TimeRange::M15,
        TimeRange::H1,
        TimeRange::H6,
        TimeRange::H24,
        TimeRange::D7,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::M15 => "15m",
            TimeRange::H1 => "1h",
            TimeRange::H6 => "6h",
            TimeRange::H24 => "24h",
            TimeRange::D7 => "7d",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.label() == label)
    }

    pub fn duration(self) -> Duration {
        Duration::from_secs(match self {
            TimeRange::M15 => 15 * 60,
            TimeRange::H1 => 3600,
            TimeRange::H6 => 6 * 3600,
            TimeRange::H24 => 24 * 3600,
            TimeRange::D7 => 7 * 24 * 3600,
        })
    }

    /// Resolution: about 120 points per chart, never finer than 15 s (a common scrape interval).
    pub fn step(self) -> Duration {
        Duration::from_secs((self.duration().as_secs() / 120).max(15))
    }

    /// How often charts of this range refresh.
    pub fn refresh(self) -> Duration {
        Duration::from_secs(match self {
            TimeRange::M15 => 15,
            TimeRange::H1 => 30,
            TimeRange::H6 => 60,
            TimeRange::H24 => 120,
            TimeRange::D7 => 600,
        })
    }

    /// `(start, end, step)` of a range query ending now. The end is rounded down to 15 s, so
    /// views asking within the same 15 s share one cached result, and the newest point is at
    /// most 15 s old (rounding to the step would hide up to 84 min on the 7-day range).
    pub fn window(self, now: f64) -> (f64, f64, f64) {
        let end = (now / 15.0).floor() * 15.0;
        (
            end - self.duration().as_secs_f64(),
            end,
            self.step().as_secs_f64(),
        )
    }
}
