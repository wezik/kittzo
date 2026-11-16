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
    version: Version,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,

    pub total: Money,
    pub details: details::Details,
    pub source: Source,
    state: State,
    ack: Ack,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ack {
    Pending,
    Accepted,
    Rejected,
}

impl Payment {
    pub fn new(total: Money, details: details::Details, source: Source) -> (Payment, Event) {
        let now = OffsetDateTime::now_utc();
        let id = Uuid::new_v4();

        let ack = match source {
            Source::Manual => Ack::Accepted,
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

    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: Uuid,
        version: Version,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
        total: Money,
        details: Details,
        source: Source,
        state: State,
        ack: Ack,
    ) -> Payment {
        Payment {
            id,
            version,
            created_at,
            updated_at,
            total,
            details,
            source,
            state,
            ack,
        }
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn ack(&self) -> Ack {
        self.ack
    }

    pub fn with_next_version(mut self) -> Payment {
        self.version = self.version.next();
        self
    }

    pub fn pay(&mut self) -> Result<Event, Error> {
        let now = OffsetDateTime::now_utc();
        let next = State::Paid { paid_at: now };
        if !matches!((&self.state, &self.ack), (State::Unpaid, Ack::Accepted)) {
            return Err(self.invalid_transition(&next));
        }
        self.state = next;
        self.updated_at = now;
        Ok(Event::Finished { id: self.id })
    }

    pub fn acknowledge(&mut self, accepted: bool) -> Result<Event, Error> {
        let next = if accepted {
            Ack::Accepted
        } else {
            Ack::Rejected
        };
        if !matches!((&self.state, &self.ack), (State::Unpaid, Ack::Pending)) {
            return Err(self.invalid_transition(&next));
        }
        self.ack = next;
        self.updated_at = OffsetDateTime::now_utc();
        Ok(Event::Acked { id: self.id })
    }

    fn invalid_transition(&self, to: &impl std::fmt::Debug) -> Error {
        Error::InvalidTransition {
            from: format!("{:?} {:?}", self.state, self.ack),
            to: format!("{to:?}"),
        }
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

    #[error("only standard details are supported here")]
    UnsupportedDetails,

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
            (some_payment_source_manual(), Ack::Accepted),
            (some_payment_source_schedule(), Ack::Pending),
            (some_payment_source_email(), Ack::Pending),
        ];

        // when
        for (source, expected) in cases {
            let (payment, _) = Payment::new(total, details.clone(), source);

            // then
            assert_eq!(payment.ack(), expected);
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
        assert_ne!(payment.ack(), Ack::Pending);

        // when
        let res = payment.acknowledge(true).expect_err("invalid ack");

        // then
        assert_matches!(res, Error::InvalidTransition { .. });
    }

    #[test]
    fn pay_requires_accepted_ack() {
        let pending = || {
            Payment::new(
                some_money(),
                some_standard_details(),
                some_payment_source_email(),
            )
            .0
        };
        let mut rejected = pending();
        rejected.acknowledge(false).unwrap();

        for mut payment in [pending(), rejected] {
            let res = payment.pay().expect_err("unaccepted pay");

            assert_matches!(res, Error::InvalidTransition { .. });
            assert_eq!(payment.state(), State::Unpaid);
        }
    }

    #[test]
    fn pay_succeeds_for_accepted_unpaid() {
        let mut payment = some_payment();
        payment.pay().expect("accepted pay");
        assert_matches!(payment.state(), State::Paid { .. });
    }

    #[test]
    fn ack_rejects_for_paid() {
        let (mut payment, _) = Payment::new(
            some_money(),
            some_standard_details(),
            some_payment_source_email(),
        );
        payment.acknowledge(true).unwrap();
        payment.pay().unwrap();

        let res = payment.acknowledge(false).expect_err("ack after pay");

        assert_matches!(res, Error::InvalidTransition { .. });
        assert_eq!(payment.ack(), Ack::Accepted);
    }
}
