//! Korean lunar dates and a local, rule-based public-holiday calendar.
//!
//! The holiday rules are based on Articles 2–3 of the Regulation on Public
//! Holidays of Government Offices. The 2026 amendment took effect in stages:
//! Labor Day on May 1, and the expanded national-holiday clause (including
//! Constitution Day) on May 11. Both are holidays in the 2026 calendar.
//! Confirmed one-off holidays are listed separately below. Future one-off
//! holidays and future legal amendments cannot be inferred offline.
//!
//! Sources:
//! https://www.law.go.kr/lsInfoP.do?lsiSeq=285779&viewCls=lsRvsDocInfoR
//! https://www.mpm.go.kr/mpm/comm/newsPress/newsPressRelease/?mode=list&searchCondition=title&searchKeyword=%EA%B3%B5%ED%9C%B4%EC%9D%BC

use crate::model::Date;
use rs_klc::LunarSolarConverter;
use std::collections::{BTreeMap, BTreeSet};

/// The date range for which this module applies a consistent modern rule set.
pub const HOLIDAY_FIRST_YEAR: i32 = 2024;
pub const HOLIDAY_LAST_YEAR: i32 = 2050;

/// Display with the calendar when a future year's holiday list is shown.
pub const HOLIDAY_DATA_NOTE: &str =
    "2024–2050년 현행 공휴일 규칙으로 계산. 향후 임시공휴일·선거일·법령 변경은 자동 반영되지 않음.";

/// Compact Korean lunar date, for example `음력 8.15` or `음력 윤2.1`.
/// Returns an empty string when the converter does not support the date.
pub fn lunar_label(date: Date) -> String {
    if date.year < 1391 || date.year > 2050 || !date.is_valid() {
        return String::new();
    }
    let mut converter = LunarSolarConverter::new();
    if !converter.set_solar_date(date.year as u32, date.month, date.day) {
        return String::new();
    }
    if converter.is_intercalation() {
        format!(
            "음력 윤{}.{}",
            converter.lunar_month(),
            converter.lunar_day()
        )
    } else {
        format!("음력 {}.{}", converter.lunar_month(), converter.lunar_day())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SubstituteRule {
    /// National holidays, Buddha's Birthday, Labor Day, Children's Day,
    /// and Christmas: Saturday, Sunday, or weekday collision.
    WeekendOrCollision,
    /// Lunar New Year and Chuseok: Sunday or weekday collision.
    SundayOrCollision,
    /// New Year's Day, Memorial Day, elections, and one-off holidays.
    None,
}

struct Holiday {
    date: Date,
    label: &'static str,
    rule: SubstituteRule,
    // Only Article 2 items 2–10 participate in a weekday collision.
    collision_group: bool,
}

fn lunar_to_solar(year: i32, month: u32, day: u32) -> Option<Date> {
    let mut converter = LunarSolarConverter::new();
    if !converter.set_lunar_date(year, month, day, false) {
        return None;
    }
    Some(Date {
        year: converter.solar_year() as i32,
        month: converter.solar_month(),
        day: converter.solar_day(),
    })
}

fn shifted(date: Date, days: i32) -> Option<Date> {
    Date::from_ordinal(date.ordinal() + days)
}

fn weekday(date: Date) -> u32 {
    // 0001-01-01 is Monday. Return Sunday=0, Saturday=6.
    (date.ordinal() + 1) as u32 % 7
}

fn add(
    holidays: &mut Vec<Holiday>,
    year: i32,
    date: Date,
    label: &'static str,
    rule: SubstituteRule,
) {
    if date.year == year {
        holidays.push(Holiday {
            date,
            label,
            rule,
            collision_group: true,
        });
    }
}

fn add_other(holidays: &mut Vec<Holiday>, year: i32, date: Date, label: &'static str) {
    if date.year == year {
        holidays.push(Holiday {
            date,
            label,
            rule: SubstituteRule::None,
            collision_group: false,
        });
    }
}

fn base_holidays(year: i32) -> Vec<Holiday> {
    let mut days = Vec::with_capacity(25);
    let fixed = [
        (1, 1, "신정", SubstituteRule::None),
        (3, 1, "삼일절", SubstituteRule::WeekendOrCollision),
        (5, 5, "어린이날", SubstituteRule::WeekendOrCollision),
        (6, 6, "현충일", SubstituteRule::None),
        (8, 15, "광복절", SubstituteRule::WeekendOrCollision),
        (10, 3, "개천절", SubstituteRule::WeekendOrCollision),
        (10, 9, "한글날", SubstituteRule::WeekendOrCollision),
        (12, 25, "성탄절", SubstituteRule::WeekendOrCollision),
    ];
    for (month, day, label, rule) in fixed {
        add(&mut days, year, Date { year, month, day }, label, rule);
    }
    if year >= 2026 {
        add(
            &mut days,
            year,
            Date {
                year,
                month: 5,
                day: 1,
            },
            "노동절",
            SubstituteRule::WeekendOrCollision,
        );
        add(
            &mut days,
            year,
            Date {
                year,
                month: 7,
                day: 17,
            },
            "제헌절",
            SubstituteRule::WeekendOrCollision,
        );
    }

    if let Some(buddha) = lunar_to_solar(year, 4, 8) {
        add(
            &mut days,
            year,
            buddha,
            "부처님 오신 날",
            SubstituteRule::WeekendOrCollision,
        );
    }
    if let Some(chuseok) = lunar_to_solar(year, 8, 15) {
        for (offset, name) in [(-1, "추석 연휴"), (0, "추석"), (1, "추석 연휴")] {
            if let Some(date) = shifted(chuseok, offset) {
                add(
                    &mut days,
                    year,
                    date,
                    name,
                    SubstituteRule::SundayOrCollision,
                );
            }
        }
    }
    // The lunar New Year's Eve can lie in the prior Gregorian year. The next
    // lunar year's Eve is included if it lands on December 31 of this year.
    for lunar_year in [year, year + 1] {
        if let Some(new_year) = lunar_to_solar(lunar_year, 1, 1) {
            for (offset, name) in [(-1, "설날 연휴"), (0, "설날"), (1, "설날 연휴")] {
                if let Some(date) = shifted(new_year, offset) {
                    add(
                        &mut days,
                        year,
                        date,
                        name,
                        SubstituteRule::SundayOrCollision,
                    );
                }
            }
        }
    }

    // Confirmed elections and ad hoc holidays. Future elections or holidays
    // must be added after the relevant government announcement.
    match year {
        2024 => {
            add_other(
                &mut days,
                year,
                Date {
                    year,
                    month: 4,
                    day: 10,
                },
                "국회의원 선거일",
            );
            add_other(
                &mut days,
                year,
                Date {
                    year,
                    month: 10,
                    day: 1,
                },
                "국군의 날 임시공휴일",
            );
        }
        2025 => {
            add_other(
                &mut days,
                year,
                Date {
                    year,
                    month: 1,
                    day: 27,
                },
                "임시공휴일",
            );
            add_other(
                &mut days,
                year,
                Date {
                    year,
                    month: 6,
                    day: 3,
                },
                "대통령 선거일",
            );
        }
        2026 => {
            add_other(
                &mut days,
                year,
                Date {
                    year,
                    month: 6,
                    day: 3,
                },
                "지방선거일",
            );
        }
        _ => {}
    }
    days
}

fn append_label(map: &mut BTreeMap<String, String>, date: Date, label: &str) {
    let current = map.entry(date.to_string()).or_default();
    if current.is_empty() {
        current.push_str(label);
    } else if !current.split(" · ").any(|part| part == label) {
        current.push_str(" · ");
        current.push_str(label);
    }
}

/// Korean public holidays keyed by `YYYY-MM-DD` for 2024–2050.
///
/// Includes substitutes under the rules in force in October 2026. Future
/// government-declared holidays and election dates are intentionally omitted.
pub fn holidays(year: i32) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    if !(HOLIDAY_FIRST_YEAR..=HOLIDAY_LAST_YEAR).contains(&year) {
        return result;
    }
    let base = base_holidays(year);
    let mut occupied = BTreeSet::new();
    let mut collision_counts: BTreeMap<Date, usize> = BTreeMap::new();
    for day in &base {
        occupied.insert(day.date);
        append_label(&mut result, day.date, day.label);
        if day.collision_group {
            *collision_counts.entry(day.date).or_default() += 1;
        }
    }

    let mut triggers = BTreeSet::new();
    for day in &base {
        let weekday = weekday(day.date);
        let collision = weekday != 0
            && weekday != 6
            && collision_counts.get(&day.date).copied().unwrap_or_default() > 1;
        let eligible = match day.rule {
            SubstituteRule::WeekendOrCollision => weekday == 0 || weekday == 6 || collision,
            SubstituteRule::SundayOrCollision => weekday == 0 || collision,
            SubstituteRule::None => false,
        };
        if eligible {
            triggers.insert(day.date);
        }
    }
    // One substitute for each distinct holiday date that met Article 3.
    // If two different dates point to the same substitute, move the second
    // forward to the next free day (Article 3(2)). Saturdays are skipped too.
    for trigger in triggers {
        let mut ordinal = trigger.ordinal() + 1;
        while let Some(date) = Date::from_ordinal(ordinal) {
            if weekday(date) != 0 && weekday(date) != 6 && !occupied.contains(&date) {
                occupied.insert(date);
                if date.year == year {
                    append_label(&mut result, date, "대체공휴일");
                }
                break;
            }
            ordinal += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lunar_dates_and_invalid_range() {
        assert_eq!(lunar_label(Date::parse("2026-02-17").unwrap()), "음력 1.1");
        assert_eq!(lunar_label(Date::parse("2026-09-25").unwrap()), "음력 8.15");
        assert_eq!(
            lunar_label(Date {
                year: 2051,
                month: 1,
                day: 1
            }),
            ""
        );
    }

    #[test]
    fn korean_holidays_2025_and_2026() {
        let previous = holidays(2025);
        assert_eq!(
            previous.get("2025-05-05").unwrap(),
            "어린이날 · 부처님 오신 날"
        );
        assert_eq!(previous.get("2025-05-06").unwrap(), "대체공휴일");
        assert!(!previous.contains_key("2025-05-07"));
        assert_eq!(previous.get("2025-01-27").unwrap(), "임시공휴일");
        let current = holidays(2026);
        assert_eq!(current.get("2026-02-17").unwrap(), "설날");
        assert_eq!(current.get("2026-09-25").unwrap(), "추석");
        assert_eq!(current.get("2026-07-17").unwrap(), "제헌절");
        assert_eq!(current.get("2026-05-01").unwrap(), "노동절");
        assert_eq!(current.get("2026-10-05").unwrap(), "대체공휴일");
        assert_eq!(current.get("2026-06-03").unwrap(), "지방선거일");
    }

    #[test]
    fn old_constitution_day_is_not_a_public_holiday() {
        assert!(!holidays(2025).contains_key("2025-07-17"));
        assert!(holidays(2051).is_empty());
    }
}
