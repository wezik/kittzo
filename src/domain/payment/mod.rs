use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::{money::Money, version::Version};

mod details;
mod event;
mod query;
pub use details::*;
pub use event::*;
pub use query::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Payment {
    pub id: Uuid,
    pub version: Version,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,

    pub total: Money,
    pub details: details::Details,
    pub source: Source,
    pub state: State,
    pub ack: Ack,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Unpaid,
    Paid { paid_at: OffsetDateTime },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Manual,
    Schedule {
        schedule_id: Uuid,
        occurrence_date: Date,
    },
    Email {
        message_id: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ack {
    Pending,
    Confirmed { value: bool },
}

impl Payment {
    pub fn new(total: Money, details: details::Details, source: Source) -> (Payment, Event) {
        let now = OffsetDateTime::now_utc();
        let id = Uuid::new_v4();

        let ack = match source {
            Source::Manual => Ack::Confirmed { value: true },
            _ => Ack::Pending,
        };

        let payment = Payment {
            id,
            version: Version::FIRST,
            created_at: now,
            updated_at: now,
            total: total,
            details: details,
            source: source,
            state: State::Unpaid,
            ack: ack,
        };
        (payment, Event::Created { id })
    }

    pub fn pay(&mut self) -> Result<Event, Error> {
        let now = OffsetDateTime::now_utc();
        let next = State::Paid { paid_at: now };

        // invariant guard
        if self.state != State::Unpaid {
            let err = Error::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", next),
            };

            return Err(err);
        }

        self.state = next;
        self.updated_at = now;

        Ok(Event::Finished { id: self.id })
    }

    pub fn ack(&mut self, accepted: bool) -> Result<Event, Error> {
        let now = OffsetDateTime::now_utc();
        let next = Ack::Confirmed { value: accepted };

        // invariant guard
        if self.ack != Ack::Pending {
            let err = Error::InvalidTransition {
                from: format!("{:?}", self.ack),
                to: format!("{:?}", next),
            };

            return Err(err);
        }

        self.ack = next;
        self.updated_at = now;
        Ok(Event::Acked { id: self.id })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("payment total is invalid")]
    InvalidTotal,

    #[error("payment title can't be empty")]
    InvalidTitle,

    #[error("payment tax form is invalid")]
    InvalidTaxForm,

    #[error("payment account number is invalid")]
    InvalidAccountNumber,

    #[error("payment recipient can't be empty")]
    InvalidRecipient,

    #[error("payment can't go from {from:?} to {to:?}")]
    InvalidTransition { from: String, to: String },
}

#[cfg(test)]
pub fn some_payment_source_manual() -> Source {
    Source::Manual
}

#[cfg(test)]
pub fn some_payment_source_schedule() -> Source {
    use time::Month;
    let some_day = Date::from_calendar_date(2020, Month::January, 20).expect("valid date");
    Source::Schedule {
        schedule_id: Uuid::new_v4(),
        occurrence_date: some_day,
    }
}

#[cfg(test)]
pub fn some_payment_source_email() -> Source {
    Source::Email {
        message_id: "some_message_id".to_string(),
    }
}

#[cfg(test)]
pub fn some_payment() -> Payment {
    use crate::domain::money::some_money;
    let some_details = some_standard_details();
    let some_source = some_payment_source_manual();
    let some_total = some_money();
    let (payment, _) = Payment::new(some_total, some_details, some_source);
    payment
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use crate::domain::money::some_money;

    use super::*;

    #[test]
    fn payments_start_with_correct_ack() {
        // given
        let total = some_money();
        let details = some_standard_details();
        let cases = vec![
            (some_payment_source_manual(), Ack::Confirmed { value: true }),
            (some_payment_source_schedule(), Ack::Pending),
            (some_payment_source_email(), Ack::Pending),
        ];

        // when
        for (source, expected) in cases {
            let (payment, _) = Payment::new(total, details.clone(), source);

            // then
            assert_eq!(payment.ack, expected);
        }
    }

    #[test]
    fn ack_rejects_for_non_pending() {
        // given
        let total = some_money();
        let details = some_standard_details();
        let source = some_payment_source_manual();
        let (mut payment, _) = Payment::new(total, details, source);

        // make sure the test case is set up
        assert_ne!(payment.ack, Ack::Pending);

        // when
        let res = payment.ack(true).expect_err("invalid ack");

        // then
        assert_matches!(res, Error::InvalidTransition { .. });
    }
}
