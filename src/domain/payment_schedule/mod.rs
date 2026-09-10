use async_trait::async_trait;
use std::time::Duration;
use time::{Date, Month, OffsetDateTime, SignedDuration};
use uuid::Uuid;

use super::money::Money;
use super::version::Version;

mod payment_schedule_job;
mod payment_schedule_service;

pub use payment_schedule_job::*;
pub use payment_schedule_service::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Recurrence {
    EveryNMonths {
        interval_months: u32,
        day_of_month: u8,
    },
}

impl Recurrence {
    /// All occurrence dates that are due (<= now's date) and not before the
    /// schedule's own creation date. Ascending order, may be empty.
    pub fn due_occurrences(&self, created_at: OffsetDateTime, now: OffsetDateTime) -> Vec<Date> {
        let start = created_at.date();
        let today = now.date();
        match self {
            Recurrence::EveryNMonths {
                interval_months,
                day_of_month,
            } => monthly_due_occurrences(start, today, *interval_months, *day_of_month),
        }
    }
}

fn monthly_due_occurrences(
    start: Date,
    today: Date,
    interval_months: u32,
    day_of_month: u8,
) -> Vec<Date> {
    // interval_months=0 would loop forever recomputing the same month;
    // clamp to 1 as a cheap guard instead of adding schedule-construction
    // validation. Add real validation if untrusted input ever builds schedules.
    let interval_months = interval_months.max(1) as i64;

    let anchor_total_months = start.year() as i64 * 12 + (start.month() as i64 - 1);

    let occurrence_for_step = |step: i64| -> Date {
        let total = anchor_total_months + step * interval_months;
        let year = total.div_euclid(12) as i32;
        let month = Month::try_from((total.rem_euclid(12) + 1) as u8).unwrap();
        let day = day_of_month.min(month.length(year)); // clamp to last day of month
        Date::from_calendar_date(year, month, day).unwrap()
    };

    // step 0 = the occurrence in the schedule's own creation month. If that
    // clamped date falls before the creation date itself (e.g. created on the
    // 15th, day_of_month = 5 -> creation-month occurrence is the 5th, which
    // is earlier), skip it and start from step 1. This is the "never
    // backfill before creation" rule applied at the boundary month; every
    // later step is a strictly later month so it never needs re-checking.
    let mut step = if occurrence_for_step(0) < start { 1 } else { 0 };

    let mut result = Vec::new();
    loop {
        let occurrence = occurrence_for_step(step);
        if occurrence > today {
            break; // dates strictly increase with step (interval_months >= 1)
        }
        result.push(occurrence);
        step += 1;
    }
    result
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaymentSchedule {
    pub id: Uuid,
    pub version: Version,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,

    pub total: Money,
    pub recurrence: Recurrence,
    pub last_run_at: Option<OffsetDateTime>,
    pub status: PaymentScheduleStatus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaymentScheduleStatus {
    Idle,
    Processing,
}

impl PaymentSchedule {
    pub fn new(total: Money, recurrence: Recurrence) -> Self {
        let now = OffsetDateTime::now_utc();
        PaymentSchedule {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            created_at: now,
            updated_at: now,

            total,
            recurrence,
            last_run_at: None,
            status: PaymentScheduleStatus::Idle,
        }
    }

    pub fn finalize_processing(&mut self) {
        let now = OffsetDateTime::now_utc();
        self.updated_at = now;
        self.last_run_at = Some(now);
        self.status = PaymentScheduleStatus::Idle;
    }

    /// Occurrences due now that haven't been fulfilled yet, determined by `last_run_at`
    pub fn pending_occurrences(&self, now: OffsetDateTime) -> Vec<Date> {
        let due = self.recurrence.due_occurrences(self.created_at, now);
        match self.last_run_at {
            Some(last) => due.into_iter().filter(|d| *d > last.date()).collect(),
            None => due,
        }
    }
}

#[async_trait]
pub trait PaymentScheduleRepository: Send + Sync {
    async fn create(&self, schedule: PaymentSchedule) -> PaymentSchedule;
    async fn update(&self, schedule: PaymentSchedule) -> PaymentSchedule;
    async fn claim_due(&self, retry_after: SignedDuration) -> Vec<PaymentSchedule>;
    async fn find_all(&self) -> Vec<PaymentSchedule>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaymentScheduleConfig {
    pub poll_interval_secs: u64,
    pub retry_after_secs: u64,
}

impl Default for PaymentScheduleConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 1800,
            retry_after_secs: 1200,
        }
    }
}

impl PaymentScheduleConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs)
    }

    pub fn retry_after(&self) -> SignedDuration {
        SignedDuration::seconds(self.retry_after_secs as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, m: u8, d: u8) -> OffsetDateTime {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d)
            .unwrap()
            .midnight()
            .assume_utc()
    }
    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    #[test]
    fn monthly_backfills_missed_occurrences_bounded_by_creation() {
        let recurrence = Recurrence::EveryNMonths {
            interval_months: 1,
            day_of_month: 5,
        };
        let due = recurrence.due_occurrences(dt(2026, 1, 15), dt(2026, 4, 2));
        // Jan 5 excluded (before creation on Jan 15); Apr 5 excluded (after "now"); Feb 5 + Mar 5 backfilled.
        assert_eq!(due, vec![date(2026, 2, 5), date(2026, 3, 5)]);
    }

    #[test]
    fn monthly_clamps_day_to_last_day_of_short_month() {
        let recurrence = Recurrence::EveryNMonths {
            interval_months: 1,
            day_of_month: 31,
        };
        let due = recurrence.due_occurrences(dt(2026, 1, 1), dt(2026, 3, 1));
        // Jan 31 as-is; Feb clamps to 28 (2026 not leap); Mar 31 not yet due.
        assert_eq!(due, vec![date(2026, 1, 31), date(2026, 2, 28)]);
    }
}
