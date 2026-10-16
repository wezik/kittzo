use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::payment::{self, AccountNumber, Error, Payment, Source};

use super::money::Money;
use super::payment::Details;
use super::version::Version;

mod recurrence;

pub use recurrence::*;

#[derive(Clone, Debug, PartialEq)]
pub struct PaymentSchedule {
    pub id: Uuid,
    pub version: Version,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,

    pub total: Money,
    pub title_template: String,

    pub next_due_at: Date,
    pub next_details: Details,

    pub recurrence: Recurrence,
}

fn next_title_from_template(template: &str, at: Date) -> String {
    template
        .replace("{MM}", &format!("{:02}", u8::from(at.month())))
        .replace("{YYYY}", &at.year().to_string())
}

impl PaymentSchedule {
    // NOTE: currently payment schedules support only regular payments
    pub fn new(
        total: Money,
        title_template: String,
        account_number: AccountNumber,
        recipient: String,
        recurrence: Recurrence,
        starts_at: Date,
    ) -> Result<Self, Error> {
        let now = OffsetDateTime::now_utc();
        let next_due_at = recurrence.next_occurrence(starts_at.previous_day().unwrap_or(starts_at));
        let next_title = next_title_from_template(&title_template, next_due_at);
        let next_details = Details::standard(next_title, account_number, recipient)?;

        let schedule = PaymentSchedule {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            created_at: now,
            updated_at: now,

            total: total,
            title_template: title_template,

            next_due_at: next_due_at,
            next_details: next_details,

            recurrence,
        };

        Ok(schedule)
    }

    pub fn produce(&mut self, today: Date) -> Result<Vec<(Payment, payment::Event)>, Error> {
        let mut produced = Vec::new();
        while self.next_due_at <= today {
            let source = Source::Schedule {
                schedule_id: self.id,
                occurrence_date: self.next_due_at,
            };
            produced.push(Payment::new(self.total, self.next_details.clone(), source));
            self.advance_due_date()?;
        }
        Ok(produced)
    }

    fn advance_due_date(&mut self) -> Result<(), Error> {
        self.next_due_at = self.recurrence.next_occurrence(self.next_due_at);
        self.updated_at = OffsetDateTime::now_utc();

        // update details
        let next_title = next_title_from_template(&self.title_template, self.next_due_at);
        let (acc_number, recipient) = match &self.next_details {
            Details::Standard {
                title: _,
                account_number,
                recipient,
            } => (account_number, recipient),
            // NOTE: Acceptable panic here this is an invariant that should not be matched unless we explicitly support it
            _ => panic!("expected standard details, no other details are supported"),
        };

        self.next_details = Details::standard(next_title, acc_number.clone(), recipient.clone())?;
        Ok(())
    }
}

#[cfg(test)]
pub fn some_schedule(title_template: &str, day_of_month: u8) -> PaymentSchedule {
    use crate::domain::money::some_money;

    PaymentSchedule::new(
        some_money(),
        title_template.into(),
        AccountNumber::from("61109010140000071219812874".into()).expect("valid account number"),
        "Landlord".into(),
        Recurrence::Monthly { day_of_month },
        OffsetDateTime::now_utc().date(),
    )
    .expect("valid schedule")
}

#[cfg(test)]
mod tests {
    use time::Month;

    use crate::domain::money::some_money;

    use super::*;

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    fn schedule_starting(day_of_month: u8, starts_at: Date) -> PaymentSchedule {
        PaymentSchedule::new(
            some_money(),
            "Foo {MM}/{YYYY}".to_string(),
            AccountNumber::from("61109010140000071219812874".to_string()).unwrap(),
            "someone".to_string(),
            Recurrence::Monthly { day_of_month },
            starts_at,
        )
        .unwrap()
    }

    #[test]
    fn template_fills_month_and_year() {
        // given
        let template = "foo {MM} bar {YYYY}";
        let cursor = Date::from_calendar_date(2025, Month::February, 2).unwrap();

        // when
        let result = next_title_from_template(&template, cursor);

        // then
        assert_eq!(result, "foo 02 bar 2025");
    }

    #[test]
    fn new_is_first_due_on_the_start_date_when_it_matches_the_day() {
        // when
        let schedule = schedule_starting(10, date(2026, 8, 10));

        // then
        assert_eq!(schedule.next_due_at, date(2026, 8, 10));
    }

    #[test]
    fn first_due_on_next_matching_day() {
        // when
        let schedule_on_starting_date = schedule_starting(10, date(2026, 8, 10));
        let schedule_after_starting_date = schedule_starting(10, date(2026, 8, 11));

        // then
        assert_eq!(schedule_on_starting_date.next_due_at, date(2026, 8, 10)); // same day
        assert_eq!(schedule_after_starting_date.next_due_at, date(2026, 9, 10)); // month later
    }

    #[test]
    fn produce_creates_one_payment_per_missed_occurrence() {
        // given
        let total = some_money();
        let mut schedule = schedule_starting(10, date(2026, 8, 1));

        // when
        let produced = schedule.produce(date(2026, 10, 16)).unwrap();

        // then
        fn eq(
            (payment, event): &(payment::Payment, payment::Event),
            expected_total: Money,
            expected_schedule_id: Uuid,
            expected_title: &str,
            expected_occurrence: Date,
        ) {
            assert_eq!(payment.total, expected_total);

            match &payment.details {
                payment::Details::Standard {
                    title,
                    account_number,
                    recipient,
                } => {
                    assert_eq!(title, expected_title);
                    assert_eq!(account_number.as_str(), "61109010140000071219812874");
                    assert_eq!(recipient, "someone");
                }
                _ => panic!("Expected Details::Standard"),
            }

            match payment.source {
                payment::Source::Schedule {
                    schedule_id,
                    occurrence_date,
                } => {
                    assert_eq!(schedule_id, expected_schedule_id);
                    assert_eq!(occurrence_date, expected_occurrence);
                }
                _ => panic!("Expected Source::Schedule"),
            }

            match event {
                payment::Event::Created { id } => assert_eq!(payment.id, *id),
                _ => panic!("Expected Event::Created"),
            }
        }

        assert_eq!(produced.len(), 3);
        eq(
            &produced[0],
            total,
            schedule.id,
            "Foo 08/2026",
            date(2026, 8, 10),
        );
        eq(
            &produced[1],
            total,
            schedule.id,
            "Foo 09/2026",
            date(2026, 9, 10),
        );
        eq(
            &produced[2],
            total,
            schedule.id,
            "Foo 10/2026",
            date(2026, 10, 10),
        );
        assert_eq!(schedule.next_due_at, date(2026, 11, 10));

        match &schedule.next_details {
            Details::Standard {
                title,
                account_number: _,
                recipient: _,
            } => assert_eq!(title, "Foo 11/2026"),
            _ => panic!("expected Details::Standard"),
        };
    }

    #[test]
    fn produce_is_empty_when_nothing_is_due() {
        // given
        let mut schedule = schedule_starting(10, date(2026, 11, 1));

        // when
        let produced = schedule.produce(date(2026, 10, 16)).unwrap();

        // then
        assert!(produced.is_empty());
        assert_eq!(schedule.next_due_at, date(2026, 11, 10));
    }
}
