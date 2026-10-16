use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::payment::{self, AccountNumber, Error, Payment, Source};

use super::money::Money;
use super::payment::Details;
use super::version::Version;

mod payment_schedule_job;
mod payment_schedule_service;
mod recurrence;

pub use payment_schedule_job::*;
pub use payment_schedule_service::*;
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
    ) -> Result<Self, Error> {
        let now = OffsetDateTime::now_utc();
        let next_due_at = recurrence.next_occurrence(now.date(), None);
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
        let start = self.created_at.date();
        let cursor = Some(self.next_due_at);
        let now = OffsetDateTime::now_utc();
        self.next_due_at = self.recurrence.next_occurrence(start, cursor);
        self.updated_at = now;

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
    PaymentSchedule::new(
        Money::from_parts("PLN", "2700.00").unwrap(),
        title_template.into(),
        AccountNumber::from("61109010140000071219812874".into()).unwrap(),
        "Landlord".into(),
        Recurrence::Monthly { day_of_month },
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use time::Month;

    use super::*;

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    fn title(details: &Details) -> &str {
        let Details::Standard { title, .. } = details else {
            panic!("expected standard details");
        };
        title
    }

    #[test]
    fn template_fills_month_and_year() {
        assert_eq!(
            next_title_from_template("Czynsz {MM}/{YYYY}", date(2026, 3, 10)),
            "Czynsz 03/2026"
        );
        assert_eq!(next_title_from_template("foo", date(2026, 3, 10)), "foo");
    }

    #[test]
    fn new_renders_the_title_for_the_first_due_date() {
        let schedule = some_schedule("Czynsz {MM}/{YYYY}", 10);

        let expected = next_title_from_template("Czynsz {MM}/{YYYY}", schedule.next_due_at);
        assert_eq!(title(&schedule.next_details), expected);
    }

    #[test]
    fn new_rejects_a_blank_recipient() {
        let result = PaymentSchedule::new(
            Money::from_parts("PLN", "1.00").unwrap(),
            "foo".into(),
            AccountNumber::from("61109010140000071219812874".into()).unwrap(),
            " ".into(),
            Recurrence::Monthly { day_of_month: 1 },
        );
        assert!(matches!(result, Err(Error::InvalidRecipient)));
    }

    #[test]
    fn produce_creates_one_payment_per_missed_occurrence() {
        let mut schedule = PaymentSchedule {
            next_due_at: date(2026, 8, 10),
            ..some_schedule("Czynsz {MM}/{YYYY}", 10)
        };
        schedule.created_at = schedule.created_at.replace_date(date(2026, 1, 1));
        schedule.next_details = Details::standard(
            "Czynsz 08/2026".into(),
            AccountNumber::from("61109010140000071219812874".into()).unwrap(),
            "Landlord".into(),
        )
        .unwrap();

        let produced = schedule.produce(date(2026, 10, 16)).unwrap();

        let titles: Vec<&str> = produced.iter().map(|(p, _)| title(&p.details)).collect();
        assert_eq!(
            titles,
            ["Czynsz 08/2026", "Czynsz 09/2026", "Czynsz 10/2026"]
        );
        let occurrences: Vec<Date> = produced
            .iter()
            .map(|(p, _)| match p.source {
                Source::Schedule {
                    schedule_id,
                    occurrence_date,
                } => {
                    assert_eq!(schedule_id, schedule.id);
                    occurrence_date
                }
                _ => panic!("expected schedule source"),
            })
            .collect();
        assert_eq!(
            occurrences,
            [date(2026, 8, 10), date(2026, 9, 10), date(2026, 10, 10)]
        );
        assert_eq!(schedule.next_due_at, date(2026, 11, 10));
        assert_eq!(title(&schedule.next_details), "Czynsz 11/2026");
    }

    #[test]
    fn produce_is_empty_when_nothing_is_due() {
        let mut schedule = PaymentSchedule {
            next_due_at: date(2026, 11, 10),
            ..some_schedule("foo", 10)
        };

        assert!(schedule.produce(date(2026, 10, 16)).unwrap().is_empty());
        assert_eq!(schedule.next_due_at, date(2026, 11, 10));
    }

    #[test]
    fn advance_moves_a_month_and_rerenders_the_title() {
        let mut schedule = PaymentSchedule {
            next_due_at: date(2026, 12, 10),
            ..some_schedule("Czynsz {MM}/{YYYY}", 10)
        };
        schedule.created_at = schedule.created_at.replace_date(date(2026, 1, 1));

        schedule.advance_due_date().unwrap();

        assert_eq!(schedule.next_due_at, date(2027, 1, 10));
        assert_eq!(title(&schedule.next_details), "Czynsz 01/2027");
    }
}
