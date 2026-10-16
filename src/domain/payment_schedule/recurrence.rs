use time::{Date, Month};

#[derive(Clone, Debug, PartialEq)]
pub enum Recurrence {
    Monthly { day_of_month: u8 },
}

impl Recurrence {
    pub fn next_occurrence(&self, start: Date, cursor: Option<Date>) -> Date {
        match self {
            Recurrence::Monthly { day_of_month } => {
                next_occurence_monthly(*day_of_month, start, cursor)
            }
        }
    }
}

fn next_occurence_monthly(day_of_month: u8, start: Date, cursor: Option<Date>) -> Date {
    let from = cursor.filter(|v| *v > start).unwrap_or(start);
    let (year, month) = (from.year(), from.month());

    // clamp to the last day of the month
    let day = day_of_month.min(month.length(year));

    // planned occurence this month
    let this_month = Date::from_calendar_date(year, month, day).expect("valid date");

    // is planned occurence this month in the future
    if this_month > from {
        return this_month;
    }

    // move to next month occurence
    let (year, month) = {
        let next = month.next();
        let year = if next == Month::January {
            year + 1
        } else {
            year
        };
        (year, next)
    };

    // clamp again
    let day = day_of_month.min(month.length(year));

    Date::from_calendar_date(year, month, day).expect("valid date")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    #[test]
    fn next_occurrence_stays_in_month_when_due_day_has_not_passed() {
        let recurrence = Recurrence::Monthly { day_of_month: 20 };
        assert_eq!(
            recurrence.next_occurrence(date(2026, 1, 15), None),
            date(2026, 1, 20)
        );
    }

    #[test]
    fn next_occurrence_rolls_to_next_month_once_due_day_has_passed() {
        let recurrence = Recurrence::Monthly { day_of_month: 5 };
        assert_eq!(
            recurrence.next_occurrence(date(2026, 1, 15), None),
            date(2026, 2, 5)
        );
    }

    #[test]
    fn next_occurrence_clamps_to_last_day_of_a_short_month() {
        let recurrence = Recurrence::Monthly { day_of_month: 31 };
        // Jan 31 -> not yet passed on Jan 1, so due date is Jan 31.
        assert_eq!(
            recurrence.next_occurrence(date(2026, 1, 1), None),
            date(2026, 1, 31)
        );
        // From Jan 31, the next occurrence clamps into February (2026 isn't leap).
        assert_eq!(
            recurrence.next_occurrence(date(2026, 1, 1), Some(date(2026, 1, 31))),
            date(2026, 2, 28)
        );
    }

    #[test]
    fn next_occurrence_advances_past_the_cursor_even_in_december() {
        let recurrence = Recurrence::Monthly { day_of_month: 10 };
        assert_eq!(
            recurrence.next_occurrence(date(2026, 1, 1), Some(date(2026, 12, 10))),
            date(2027, 1, 10)
        );
    }
}
