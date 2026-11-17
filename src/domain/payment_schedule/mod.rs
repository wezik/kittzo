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
    pub fn new(
        total: Money,
        title_template: String,
        account_number: AccountNumber,
        recipient: String,
        recurrence: Recurrence,
        starts_at: Date,
    ) -> Result<Self, Error> {
        if !matches!(
            recurrence,
            Recurrence::Monthly {
                day_of_month: 1..=31
            }
        ) {
            return Err(Error::InvalidDayOfMonth);
        }
        let now = OffsetDateTime::now_utc();
        let next_due_at = recurrence.next_occurrence(starts_at.previous_day().unwrap_or(starts_at));
        let next_title = next_title_from_template(&title_template, next_due_at);
        let next_details = Details::standard(next_title, account_number, recipient)?;

        let schedule = PaymentSchedule {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            created_at: now,
            updated_at: now,

            total,
            title_template,

            next_due_at,
            next_details,

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

        let next_title = next_title_from_template(&self.title_template, self.next_due_at);
        let Details::Standard {
            account_number,
            recipient,
            ..
        } = &self.next_details
        else {
            return Err(Error::UnsupportedDetails);
        };

        self.next_details =
            Details::standard(next_title, account_number.clone(), recipient.clone())?;
        Ok(())
    }
}

#[cfg(test)]
pub fn some_schedule(title_template: &str, day_of_month: u8) -> PaymentSchedule {
    use crate::domain::money::some_money;

    PaymentSchedule::new(
        some_money(),
        title_template.into(),
        AccountNumber::try_from("61109010140000071219812874".to_string())
            .expect("valid account number"),
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
            AccountNumber::try_from("61109010140000071219812874".to_string()).unwrap(),
            "someone".to_string(),
            Recurrence::Monthly { day_of_month },
            starts_at,
        )
        .unwrap()
    }

    #[test]
    fn template_fills_month_and_year() {
        let template = "foo {MM} bar {YYYY}";
        let cursor = Date::from_calendar_date(2025, Month::February, 2).unwrap();

        let result = next_title_from_template(template, cursor);

        assert_eq!(result, "foo 02 bar 2025");
    }

    #[test]
    fn new_rejects_day_of_month_out_of_range() {
        for day_of_month in [0, 32] {
            let result = PaymentSchedule::new(
                some_money(),
                "Foo".to_string(),
                AccountNumber::try_from("61109010140000071219812874".to_string()).unwrap(),
                "someone".to_string(),
                Recurrence::Monthly { day_of_month },
                date(2026, 8, 10),
            );

            assert!(matches!(result, Err(Error::InvalidDayOfMonth)));
        }
    }

    #[test]
    fn new_is_first_due_on_the_start_date_when_it_matches_the_day() {
        let schedule = schedule_starting(10, date(2026, 8, 10));

        assert_eq!(schedule.next_due_at, date(2026, 8, 10));
    }

    #[test]
    fn first_due_on_next_matching_day() {
        let schedule_on_starting_date = schedule_starting(10, date(2026, 8, 10));
        let schedule_after_starting_date = schedule_starting(10, date(2026, 8, 11));

        assert_eq!(schedule_on_starting_date.next_due_at, date(2026, 8, 10));
        assert_eq!(schedule_after_starting_date.next_due_at, date(2026, 9, 10));
    }

    #[test]
    fn produce_creates_one_payment_per_missed_occurrence() {
        let total = some_money();
        let mut schedule = schedule_starting(10, date(2026, 8, 1));
        let schedule_id = schedule.id;
        let details = |title: &str| {
            Details::standard(
                title.to_string(),
                AccountNumber::try_from("61109010140000071219812874".to_string()).unwrap(),
                "someone".to_string(),
            )
            .unwrap()
        };
        let occurrence = |occurrence_date| Source::Schedule {
            schedule_id,
            occurrence_date,
        };

        let produced = schedule.produce(date(2026, 10, 16)).unwrap();

        for (payment, event) in &produced {
            assert_eq!(payment.total, total);
            assert_eq!(*event, payment::Event::Created { id: payment.id });
        }
        assert_eq!(
            produced
                .iter()
                .map(|(payment, _)| (payment.details.clone(), payment.source.clone()))
                .collect::<Vec<_>>(),
            vec![
                (details("Foo 08/2026"), occurrence(date(2026, 8, 10))),
                (details("Foo 09/2026"), occurrence(date(2026, 9, 10))),
                (details("Foo 10/2026"), occurrence(date(2026, 10, 10))),
            ]
        );
        assert_eq!(schedule.next_due_at, date(2026, 11, 10));
        assert_eq!(schedule.next_details, details("Foo 11/2026"));
    }

    #[test]
    fn produce_is_empty_when_nothing_is_due() {
        let mut schedule = schedule_starting(10, date(2026, 11, 1));

        let produced = schedule.produce(date(2026, 10, 16)).unwrap();

        assert!(produced.is_empty());
        assert_eq!(schedule.next_due_at, date(2026, 11, 10));
    }
}
