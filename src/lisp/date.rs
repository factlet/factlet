use std::fmt;

/// A calendar date, written `2025-12-31`. Ordered chronologically.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    /// `None` unless it's a real date: no February 30th.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Date> {
        let valid = (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month);
        valid.then_some(Date { year, month, day })
    }

    /// `Some` if `text` is shaped like `YYYY-MM-DD`, `Err` if that isn't a
    /// real date.
    pub fn parse(text: &str) -> Option<Result<Date, String>> {
        let b = text.as_bytes();
        let shaped = b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
        if !shaped {
            return None;
        }
        let (y, m, d) = (&text[..4], &text[5..7], &text[8..]);
        let date = Date::new(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
        Some(date.ok_or_else(|| format!("`{text}` is not a real date")))
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }

    /// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
    pub fn days(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Whole months from `self` to `later`, counting a month only once its
    /// day of the month is reached; negative if `later` is earlier.
    pub fn months_until(self, later: Date) -> i64 {
        if later < self {
            return -later.months_until(self);
        }
        let months =
            i64::from(later.year - self.year) * 12 + i64::from(later.month) - i64::from(self.month);
        // Jan 31 to Feb 28 is a whole month: the 31st doesn't exist then.
        let reached = later.day >= self.day.min(days_in_month(later.year, later.month));
        months - i64::from(!reached)
    }

    /// Age in whole years on `on`, having turned a year older on each
    /// birthday (Feb 29 birthdays on Feb 28 in other years).
    pub fn age_on(self, on: Date) -> i64 {
        self.months_until(on).div_euclid(12)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}
