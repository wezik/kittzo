use async_trait::async_trait;
use time::{Date, Month, OffsetDateTime, SignedDuration};
use uuid::Uuid;

use super::money::Money;
use super::payment::Details;
use super::version::Version;

mod payment_schedule_job;
mod payment_schedule_service;

pub use payment_schedule_job::*;
pub use payment_schedule_service::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Recurrence {
    Monthly { day_of_month: u8 },
}

impl Recurrence {
    /// The next date this recurrence is due, strictly after `cursor` (or at/after `start`
    /// if `cursor` is `None` or not later than `start`).
    pub fn next_occurrence(&self, start: Date, cursor: Option<Date>) -> Date {
        match self {
            Recurrence::Monthly { day_of_month } => {
                let from = cursor.filter(|v| *v > start).unwrap_or(start);
                let this_month = monthly_date_in(from.year(), from.month(), *day_of_month);

                if this_month > from {
                    this_month
                } else {
                    let (year, month) = next_month(from.year(), from.month());
                    monthly_date_in(year, month, *day_of_month)
                }
            }
        }
    }
}

// clamps to the last day of the month, e.g. day_of_month = 31 in February -> the 28th (or 29th).
fn monthly_date_in(year: i32, month: Month, day_of_month: u8) -> Date {
    let day = day_of_month.min(month.length(year));
    Date::from_calendar_date(year, month, day).expect("failed to produce date")
}

fn next_month(year: i32, month: Month) -> (i32, Month) {
    let next = month.next();
    let year = if next == Month::January {
        year + 1
    } else {
        year
    };
    (year, next)
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaymentSchedule {
    pub id: Uuid,
    pub version: Version,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub next_due_at: Date,

    pub total: Money,
    pub details: Details,
    pub recurrence: Recurrence,
    pub status: PaymentScheduleStatus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaymentScheduleStatus {
    Idle,
    Processing,
}

impl PaymentSchedule {
    pub fn new(total: Money, details: Details, recurrence: Recurrence) -> Self {
        let now = OffsetDateTime::now_utc();
        let next_due_at = recurrence.next_occurrence(now.date(), None);
        PaymentSchedule {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            created_at: now,
            updated_at: now,
            next_due_at,

            total,
            details,
            recurrence,
            status: PaymentScheduleStatus::Idle,
        }
    }

    pub fn details_for(&self, occurrence: Date) -> Details {
        let mut details = self.details.clone();
        if let Details::Standard { title, .. } = &mut details {
            *title = title
                .replace("{MM}", &format!("{:02}", u8::from(occurrence.month())))
                .replace("{YYYY}", &occurrence.year().to_string());
        }
        details
    }

    pub fn edit(
        &mut self,
        total: Money,
        details: Details,
        recurrence: Recurrence,
    ) -> Result<(), PaymentScheduleError> {
        if self.status == PaymentScheduleStatus::Processing {
            return Err(PaymentScheduleError::ConcurrentUpdate);
        }
        let Recurrence::Monthly { day_of_month } = recurrence;
        self.next_due_at = monthly_date_in(
            self.next_due_at.year(),
            self.next_due_at.month(),
            day_of_month,
        );
        self.total = total;
        self.details = details;
        self.recurrence = recurrence;
        self.updated_at = OffsetDateTime::now_utc();
        Ok(())
    }

    pub fn advance_due_date(&mut self) {
        self.next_due_at = self
            .recurrence
            .next_occurrence(self.created_at.date(), Some(self.next_due_at));
    }
}

#[async_trait]
pub trait PaymentScheduleRepository: Send + Sync {
    async fn create(
        &self,
        schedule: PaymentSchedule,
    ) -> Result<PaymentSchedule, PaymentScheduleError>;
    async fn update(
        &self,
        schedule: PaymentSchedule,
    ) -> Result<PaymentSchedule, PaymentScheduleError>;
    async fn claim_due(
        &self,
        retry_after: SignedDuration,
    ) -> Result<Vec<PaymentSchedule>, PaymentScheduleError>;
    async fn find_all(&self) -> Result<Vec<PaymentSchedule>, PaymentScheduleError>;
    async fn delete(&self, id: Uuid) -> Result<(), PaymentScheduleError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::payment::some_standard_details;

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    fn schedule_due(next_due_at: Date) -> PaymentSchedule {
        PaymentSchedule {
            next_due_at,
            ..PaymentSchedule::new(
                Money::from_parts("PLN", "2700.00").unwrap(),
                some_standard_details(),
                Recurrence::Monthly { day_of_month: 5 },
            )
        }
    }

    #[test]
    fn edit_moves_the_day_within_the_month_already_due() {
        let mut schedule = schedule_due(date(2026, 11, 5));

        schedule
            .edit(
                Money::from_parts("PLN", "27000.00").unwrap(),
                some_standard_details(),
                Recurrence::Monthly { day_of_month: 7 },
            )
            .unwrap();

        assert_eq!(schedule.next_due_at, date(2026, 11, 7));
        assert_eq!(
            schedule.total,
            Money::from_parts("PLN", "27000.00").unwrap()
        );
    }

    #[test]
    fn edit_clamps_the_day_to_a_short_month() {
        let mut schedule = schedule_due(date(2027, 2, 5));
        let (total, details) = (schedule.total, schedule.details.clone());

        schedule
            .edit(total, details, Recurrence::Monthly { day_of_month: 31 })
            .unwrap();

        assert_eq!(schedule.next_due_at, date(2027, 2, 28));
    }

    #[test]
    fn edit_is_refused_while_the_job_is_processing_it() {
        let mut schedule = schedule_due(date(2026, 11, 5));
        schedule.status = PaymentScheduleStatus::Processing;
        let (total, details) = (schedule.total, schedule.details.clone());

        let err = schedule
            .edit(total, details, Recurrence::Monthly { day_of_month: 7 })
            .unwrap_err();

        assert!(matches!(err, PaymentScheduleError::ConcurrentUpdate));
    }

    fn title(details: Details) -> String {
        let Details::Standard { title, .. } = details else {
            panic!("expected standard details");
        };
        title
    }

    #[test]
    fn details_for_fills_the_occurrence_month_into_the_title() {
        let schedule = PaymentSchedule::new(
            Money::from_parts("PLN", "27000.00").unwrap(),
            Details::standard(
                "Opłata za wynajem mieszkania {MM}/{YYYY}".into(),
                "61109010140000071219812874".into(),
                "Landlord".into(),
            )
            .unwrap(),
            Recurrence::Monthly { day_of_month: 10 },
        );

        assert_eq!(
            title(schedule.details_for(date(2026, 10, 10))),
            "Opłata za wynajem mieszkania 10/2026"
        );
    }

    #[test]
    fn details_for_keeps_a_title_without_placeholders() {
        let schedule = PaymentSchedule::new(
            Money::from_parts("PLN", "1.00").unwrap(),
            some_standard_details(),
            Recurrence::Monthly { day_of_month: 10 },
        );
        assert_eq!(title(schedule.details_for(date(2027, 1, 10))), "foo");
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
