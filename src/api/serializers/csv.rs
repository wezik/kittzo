use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Iso8601;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::details::TaxFormRepr;
use crate::domain::money::Money;
use crate::domain::payment::{AccountNumber, Ack, Details, Payment, Source, State};
use crate::domain::payment_schedule::{PaymentSchedule, Recurrence};
use crate::domain::version::Version;

type RowError = Box<dyn std::error::Error + Send + Sync>;

pub fn response<R: Serialize>(filename: &str, rows: impl IntoIterator<Item = R>) -> Response {
    match write(rows) {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ],
            body,
        )
            .into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to write csv");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn write<R: Serialize>(rows: impl IntoIterator<Item = R>) -> Result<Vec<u8>, ::csv::Error> {
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    for row in rows {
        writer.serialize(row)?;
    }
    writer.into_inner().map_err(|err| err.into_error().into())
}

pub fn read<R, T>(body: &[u8]) -> Result<Vec<T>, String>
where
    R: DeserializeOwned,
    T: TryFrom<R, Error = RowError>,
{
    ::csv::Reader::from_reader(body)
        .deserialize::<R>()
        .enumerate()
        .map(|(index, row)| {
            row.map_err(RowError::from)
                .and_then(T::try_from)
                .map_err(|err| format!("row {}: {err}", index + 1))
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
pub struct PaymentRow {
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    amount: String,
    currency: String,
    r#type: String,
    title: Option<String>,
    account_number: Option<String>,
    recipient: Option<String>,
    form_symbol: Option<TaxFormRepr>,
    status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    paid_at: Option<OffsetDateTime>,
    ack: String,
    source: String,
    schedule_id: Option<Uuid>,
    occurrence_date: Option<String>,
    email_message_id: Option<String>,
}

impl From<Payment> for PaymentRow {
    fn from(payment: Payment) -> Self {
        let state = payment.state();
        let ack = payment.ack();
        let (kind, title, account_number, recipient, form_symbol) = match payment.details {
            Details::Standard {
                title,
                account_number,
                recipient,
            } => (
                "standard",
                Some(title),
                Some(account_number.as_str().to_string()),
                Some(recipient),
                None,
            ),
            Details::Tax { form } => ("tax", None, None, None, Some(form.into())),
        };
        let (status, paid_at) = match state {
            State::Unpaid => ("unpaid", None),
            State::Paid { paid_at } => ("paid", Some(paid_at)),
        };
        let ack = match ack {
            Ack::Pending => "pending",
            Ack::Accepted => "accepted",
            Ack::Rejected => "rejected",
        };
        let (source, schedule_id, occurrence_date, email_message_id) = match payment.source {
            Source::Manual => ("manual", None, None, None),
            Source::Schedule {
                schedule_id,
                occurrence_date,
            } => (
                "schedule",
                Some(schedule_id),
                Some(occurrence_date.to_string()),
                None,
            ),
            Source::Email { message_id } => ("email", None, None, Some(message_id)),
        };
        PaymentRow {
            id: payment.id,
            created_at: payment.created_at,
            updated_at: payment.updated_at,
            amount: payment.total.amount().to_string(),
            currency: payment.total.currency_code().to_string(),
            r#type: kind.into(),
            title,
            account_number,
            recipient,
            form_symbol,
            status: status.into(),
            paid_at,
            ack: ack.into(),
            source: source.into(),
            schedule_id,
            occurrence_date,
            email_message_id,
        }
    }
}

impl TryFrom<PaymentRow> for Payment {
    type Error = RowError;

    fn try_from(row: PaymentRow) -> Result<Self, RowError> {
        let total = Money::from_parts(&row.currency, &row.amount)?;
        let details = match (row.r#type.as_str(), row.form_symbol) {
            ("standard", _) => Details::standard(
                row.title.ok_or("missing title")?,
                AccountNumber::try_from(row.account_number.ok_or("missing account_number")?)?,
                row.recipient.ok_or("missing recipient")?,
            )?,
            ("tax", Some(form)) => Details::tax(form.into()),
            ("tax", None) => return Err("missing form_symbol".into()),
            (other, _) => return Err(format!("unknown type {other}").into()),
        };
        let state = match (row.status.as_str(), row.paid_at) {
            ("unpaid", _) => State::Unpaid,
            ("paid", Some(paid_at)) => State::Paid { paid_at },
            ("paid", None) => return Err("missing paid_at".into()),
            (other, _) => return Err(format!("unknown status {other}").into()),
        };
        let ack = match row.ack.as_str() {
            "pending" => Ack::Pending,
            "accepted" => Ack::Accepted,
            "rejected" => Ack::Rejected,
            other => return Err(format!("unknown ack {other}").into()),
        };
        let source = match row.source.as_str() {
            "manual" => Source::Manual,
            "schedule" => Source::Schedule {
                schedule_id: row.schedule_id.ok_or("missing schedule_id")?,
                occurrence_date: parse_date(row.occurrence_date.ok_or("missing occurrence_date")?)?,
            },
            "email" => Source::Email {
                message_id: row.email_message_id.ok_or("missing email_message_id")?,
            },
            other => return Err(format!("unknown source {other}").into()),
        };
        Ok(Payment::restore(
            row.id,
            Version::FIRST,
            row.created_at,
            row.updated_at,
            total,
            details,
            source,
            state,
            ack,
        ))
    }
}

#[derive(Serialize, Deserialize)]
pub struct ScheduleRow {
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    amount: String,
    currency: String,
    title_template: String,
    account_number: String,
    recipient: String,
    day_of_month: u8,
    next_due_at: String,
}

impl From<PaymentSchedule> for ScheduleRow {
    fn from(schedule: PaymentSchedule) -> Self {
        let (account_number, recipient) = match schedule.next_details {
            Details::Standard {
                account_number,
                recipient,
                ..
            } => (account_number.as_str().to_string(), recipient),
            Details::Tax { .. } => Default::default(),
        };
        let Recurrence::Monthly { day_of_month } = schedule.recurrence;
        ScheduleRow {
            id: schedule.id,
            created_at: schedule.created_at,
            updated_at: schedule.updated_at,
            amount: schedule.total.amount().to_string(),
            currency: schedule.total.currency_code().to_string(),
            title_template: schedule.title_template,
            account_number,
            recipient,
            day_of_month,
            next_due_at: schedule.next_due_at.to_string(),
        }
    }
}

impl TryFrom<ScheduleRow> for PaymentSchedule {
    type Error = RowError;

    fn try_from(row: ScheduleRow) -> Result<Self, RowError> {
        Ok(PaymentSchedule::restore(
            row.id,
            row.created_at,
            row.updated_at,
            Money::from_parts(&row.currency, &row.amount)?,
            row.title_template,
            AccountNumber::try_from(row.account_number)?,
            row.recipient,
            Recurrence::Monthly {
                day_of_month: row.day_of_month,
            },
            parse_date(row.next_due_at)?,
        )?)
    }
}

fn parse_date(raw: String) -> Result<Date, RowError> {
    Ok(Date::parse(&raw, &Iso8601::DATE)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::some_money;
    use crate::domain::payment::{
        TaxForm, some_payment, some_payment_source_email, some_payment_source_schedule,
    };
    use crate::domain::payment_schedule::some_schedule;

    fn round_trip<R, T>(items: Vec<T>) -> Vec<T>
    where
        R: Serialize + DeserializeOwned + From<T>,
        T: TryFrom<R, Error = RowError>,
    {
        read::<R, T>(&write(items.into_iter().map(R::from)).unwrap()).unwrap()
    }

    #[test]
    fn payments_round_trip() {
        let mut paid = some_payment();
        paid.pay().unwrap();
        let (tax, _) = Payment::new(
            some_money(),
            Details::tax(TaxForm::Vat7),
            some_payment_source_email(),
        );
        let (mut rejected, _) = Payment::new(
            some_money(),
            Details::tax(TaxForm::Ppl),
            some_payment_source_schedule(),
        );
        rejected.acknowledge(false).unwrap();
        let payments = vec![some_payment(), paid, tax, rejected];

        assert_eq!(round_trip::<PaymentRow, _>(payments.clone()), payments);
    }

    #[test]
    fn schedules_round_trip() {
        let schedules = vec![some_schedule("Czynsz {MM}/{YYYY}", 31)];

        assert_eq!(round_trip::<ScheduleRow, _>(schedules.clone()), schedules);
    }

    #[test]
    fn invalid_row_is_reported_with_its_number() {
        let body = write([some_payment(), some_payment()].map(PaymentRow::from)).unwrap();
        let body =
            String::from_utf8(body)
                .unwrap()
                .replacen("61109010140000071219812874", "123", 1);

        let err = read::<PaymentRow, Payment>(body.as_bytes()).unwrap_err();

        assert!(err.starts_with("row 1:"), "{err}");
    }
}
