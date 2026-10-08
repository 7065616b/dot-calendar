use std::fmt;

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    pub fn parse(value: &str) -> Result<Self, String> {
        let bytes = value.as_bytes();
        if bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || bytes
                .iter()
                .enumerate()
                .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
        {
            return Err("date must use YYYY-MM-DD".into());
        }
        let date = Self {
            year: value[0..4].parse().map_err(|_| "invalid year")?,
            month: value[5..7].parse().map_err(|_| "invalid month")?,
            day: value[8..10].parse().map_err(|_| "invalid day")?,
        };
        if !date.is_valid() {
            return Err("date is outside the calendar".into());
        }
        Ok(date)
    }

    pub(crate) fn is_valid(self) -> bool {
        (1..=9999).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=days_in_month(self.year, self.month)).contains(&self.day)
    }

    fn next_day(self) -> Self {
        if self.day < days_in_month(self.year, self.month) {
            Self {
                day: self.day + 1,
                ..self
            }
        } else if self.month < 12 {
            Self {
                month: self.month + 1,
                day: 1,
                ..self
            }
        } else {
            Self {
                year: self.year + 1,
                month: 1,
                day: 1,
            }
        }
    }

    pub fn today() -> Self {
        // GetLocalTime uses the user's Windows timezone rather than UTC.
        let mut value: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut value) };
        Self {
            year: i32::from(value.wYear),
            month: u32::from(value.wMonth),
            day: u32::from(value.wDay),
        }
    }

    /// Number of days since 0001-01-01 in the proleptic Gregorian calendar.
    pub fn ordinal(self) -> i32 {
        let prior = self.year - 1;
        let mut days = prior * 365 + prior / 4 - prior / 100 + prior / 400;
        for month in 1..self.month {
            days += days_in_month(self.year, month) as i32;
        }
        days + self.day as i32 - 1
    }

    pub fn from_ordinal(ordinal: i32) -> Option<Self> {
        const MAX_ORDINAL: i32 = 3_652_058; // 9999-12-31
        if !(0..=MAX_ORDINAL).contains(&ordinal) {
            return None;
        }
        let (mut low, mut high) = (1, 9999);
        while low < high {
            let mid = (low + high + 1) / 2;
            let prior = mid - 1;
            let start = prior * 365 + prior / 4 - prior / 100 + prior / 400;
            if start <= ordinal {
                low = mid
            } else {
                high = mid - 1
            }
        }
        let year = low;
        let prior = year - 1;
        let mut remaining = ordinal - (prior * 365 + prior / 4 - prior / 100 + prior / 400);
        let mut month = 1;
        while remaining >= days_in_month(year, month) as i32 {
            remaining -= days_in_month(year, month) as i32;
            month += 1;
        }
        Some(Self {
            year,
            month,
            day: remaining as u32 + 1,
        })
    }
}

impl fmt::Display for Date {
    fn fmt(&self, writer: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(writer, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub id: String,
    pub date: String,
    pub time: Option<String>,
    pub title: String,
    pub notes: String,
    pub request_id: Option<String>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub recurrence: Option<String>,
    #[serde(default)]
    pub reminder_minutes: Option<u32>,
}

/// Expand recurring series into a closed date range without persisting copies.
/// Each occurrence retains the source event ID; edits and completion affect the whole series.
/// Monthly events on the 29th–31st skip months without that day; Feb 29 yearly events
/// occur only in leap years. The event's date is replaced with the occurrence date.
pub fn occurrences(events: &[Event], from: Date, to: Date) -> Vec<Event> {
    occurrence_indices(events, from, to)
        .into_iter()
        .map(|(date, index)| {
            let mut event = events[index].clone();
            event.date = date.to_string();
            event
        })
        .collect()
}

/// Expand occurrence dates while borrowing source events by index. GUI cells can
/// render large notes without cloning their strings for every repeated date.
pub fn occurrence_indices(events: &[Event], from: Date, to: Date) -> Vec<(Date, usize)> {
    let mut result = Vec::new();
    for_each_occurrence(events, from, to, |date, index| result.push((date, index)));
    result.sort_by(|(a_date, a_index), (b_date, b_index)| {
        (
            a_date,
            &events[*a_index].time,
            &events[*a_index].title,
            &events[*a_index].id,
        )
            .cmp(&(
                b_date,
                &events[*b_index].time,
                &events[*b_index].title,
                &events[*b_index].id,
            ))
    });
    result
}

/// Visit occurrences in source event order, without allocating or sorting an
/// intermediate result. The index refers to the original `events` slice.
pub fn for_each_occurrence(
    events: &[Event],
    from: Date,
    to: Date,
    mut visit: impl FnMut(Date, usize),
) {
    if from > to || !from.is_valid() || !to.is_valid() {
        return;
    }
    let last_day = to.ordinal();
    for (index, event) in events.iter().enumerate() {
        let Ok(start) = Date::parse(&event.date) else {
            continue;
        };
        if start > to {
            continue;
        }
        let lower = from.max(start);
        match event.recurrence.as_deref() {
            None => {
                if start >= from {
                    visit(start, index);
                }
            }
            Some("daily") => {
                let mut date = lower;
                loop {
                    visit(date, index);
                    if date == to {
                        break;
                    }
                    date = date.next_day();
                }
            }
            Some("weekly") => {
                let step = 7;
                let start_day = start.ordinal();
                let mut day = lower.ordinal();
                let remainder = (day - start_day) % step;
                if remainder != 0 {
                    day += step - remainder;
                }
                while day <= last_day {
                    if let Some(date) = Date::from_ordinal(day) {
                        visit(date, index);
                    }
                    day += step;
                }
            }
            Some("monthly") => {
                let mut year = lower.year;
                let mut month = lower.month;
                while year < to.year || (year == to.year && month <= to.month) {
                    if start.day <= days_in_month(year, month) {
                        let date = Date {
                            year,
                            month,
                            day: start.day,
                        };
                        if date >= lower && date <= to {
                            visit(date, index);
                        }
                    }
                    month += 1;
                    if month == 13 {
                        month = 1;
                        year += 1;
                    }
                }
            }
            Some("yearly") => {
                for year in lower.year..=to.year {
                    if start.day <= days_in_month(year, start.month) {
                        let date = Date {
                            year,
                            month: start.month,
                            day: start.day,
                        };
                        if date >= lower && date <= to {
                            visit(date, index);
                        }
                    }
                }
            }
            Some(_) => {} // The store rejects unknown recurrence values.
        }
    }
}

/// Limit public queries to one year so a daily series cannot allocate millions of rows.
pub fn validate_occurrence_range(from: Date, to: Date) -> Result<(), String> {
    if from > to {
        return Err("from must be on or before to".into());
    }
    if to.ordinal() - from.ordinal() > 366 {
        return Err("occurrence range cannot exceed 366 days".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_years_and_boundaries() {
        assert!(Date::parse("2000-02-29").is_ok());
        assert!(Date::parse("2024-02-29").is_ok());
        assert!(Date::parse("1900-02-29").is_err());
        assert!(Date::parse("2025-04-31").is_err());
        assert!(Date::parse("0000-01-01").is_err());
        assert!(Date::parse("9999-12-31").is_ok());
        assert_eq!(Date::parse("2026-10-07").unwrap().to_string(), "2026-10-07");
    }

    #[test]
    fn rejects_noncanonical_dates() {
        for value in [
            "2026-1-07",
            "2026/10/07",
            "2026-13-01",
            "2026-00-01",
            "2026-10-00",
            "2026-10-7a",
        ] {
            assert!(Date::parse(value).is_err(), "accepted {value}");
        }
    }

    fn recurring(date: &str, recurrence: &str) -> Event {
        Event {
            id: "series-1".into(),
            date: date.into(),
            time: None,
            title: "Repeat".into(),
            notes: String::new(),
            request_id: None,
            completed: false,
            color: None,
            recurrence: Some(recurrence.into()),
            reminder_minutes: None,
        }
    }

    #[test]
    fn ordinal_round_trip_and_recurrence_boundaries() {
        for date in [
            "0001-01-01",
            "1900-03-01",
            "2000-02-29",
            "2026-10-07",
            "9999-12-31",
        ] {
            let date = Date::parse(date).unwrap();
            assert_eq!(Date::from_ordinal(date.ordinal()), Some(date));
        }
        let from = Date::parse("2026-02-01").unwrap();
        let to = Date::parse("2026-03-31").unwrap();
        let monthly = occurrences(&[recurring("2026-01-31", "monthly")], from, to);
        assert_eq!(
            monthly
                .iter()
                .map(|event| event.date.as_str())
                .collect::<Vec<_>>(),
            ["2026-03-31"]
        );
        assert_eq!(monthly[0].id, "series-1");
        let yearly = occurrences(
            &[recurring("2024-02-29", "yearly")],
            Date::parse("2025-01-01").unwrap(),
            Date::parse("2028-12-31").unwrap(),
        );
        assert_eq!(
            yearly
                .iter()
                .map(|event| event.date.as_str())
                .collect::<Vec<_>>(),
            ["2028-02-29"]
        );
    }

    #[test]
    fn weekly_and_daily_omit_days_before_start() {
        let from = Date::parse("2026-10-01").unwrap();
        let to = Date::parse("2026-10-14").unwrap();
        let weekly = occurrences(&[recurring("2026-10-07", "weekly")], from, to);
        assert_eq!(
            weekly
                .iter()
                .map(|event| event.date.as_str())
                .collect::<Vec<_>>(),
            ["2026-10-07", "2026-10-14"]
        );
        let daily = occurrences(&[recurring("2026-10-13", "daily")], from, to);
        assert_eq!(daily.len(), 2);
    }

    #[test]
    fn daily_occurrences_cross_leap_month_year_and_upper_bound() {
        for (start, end, expected) in [
            (
                "2024-02-28",
                "2024-03-01",
                vec!["2024-02-28", "2024-02-29", "2024-03-01"],
            ),
            (
                "2026-12-31",
                "2027-01-02",
                vec!["2026-12-31", "2027-01-01", "2027-01-02"],
            ),
            ("9999-12-30", "9999-12-31", vec!["9999-12-30", "9999-12-31"]),
        ] {
            let events = [recurring(start, "daily")];
            let dates = occurrence_indices(
                &events,
                Date::parse(start).unwrap(),
                Date::parse(end).unwrap(),
            )
            .into_iter()
            .map(|(date, _)| date.to_string())
            .collect::<Vec<_>>();
            assert_eq!(dates, expected);
        }
    }

    #[test]
    fn indexed_occurrences_preserve_order_and_source_events() {
        let mut weekly = recurring("2026-10-07", "weekly");
        weekly.id = "weekly".into();
        weekly.time = Some("09:00".into());
        weekly.notes = "long note ".repeat(500);
        let mut single = recurring("2026-10-14", "daily");
        single.id = "single".into();
        single.recurrence = None;
        single.time = Some("08:00".into());
        let events = vec![weekly.clone(), single];
        let from = Date::parse("2026-10-07").unwrap();
        let to = Date::parse("2026-10-14").unwrap();
        let mut visited = Vec::new();
        for_each_occurrence(&events, from, to, |date, index| visited.push((date, index)));
        assert_eq!(visited, vec![(from, 0), (to, 0), (to, 1)]);
        let indexed = occurrence_indices(&events, from, to);
        assert_eq!(indexed, vec![(from, 0), (to, 1), (to, 0)]);
        let expanded = occurrences(&events, from, to);
        assert_eq!(
            expanded
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            ["weekly", "single", "weekly"]
        );
        assert_eq!(expanded[2].notes, weekly.notes);
        assert_eq!(events[0].date, "2026-10-07");
        assert!(occurrence_indices(&events, to, from).is_empty());
        assert!(occurrences(&events, to, from).is_empty());
        for_each_occurrence(&events, to, from, |date, index| visited.push((date, index)));
        assert_eq!(visited.len(), 3);
    }
}
