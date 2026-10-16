use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use serde_json::{Value, json};

use crate::domain::money::Money;
use crate::domain::payment::{AccountNumber, Details, TaxForm};

pub mod sqlx_outbox_repository;
pub mod sqlx_payment_repository;
pub mod sqlx_payment_schedule_repository;
pub mod sqlx_processed_email_repository;

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("already exists")]
    AlreadyExists,

    #[error("not found")]
    NotFound,

    #[error("updated concurrently")]
    ConcurrentUpdate,

    #[error("unexpected persistence error")]
    Unexpected,
}

pub(crate) fn unexpected(sqlx_err: sqlx::Error) -> Error {
    tracing::error!(%sqlx_err, "unexpected sqlx error");
    Error::Unexpected
}

// one connection avoids `SQLITE_BUSY` contention between writers.
pub async fn connect(url: &str) -> sqlx::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    sqlx::migrate!("resources/migrations").run(&pool).await?;

    Ok(pool)
}

/// `None` for a row written before `Money` validated precision: the row is skipped and
/// logged rather than crashing a request or a job loop over historical data.
pub(crate) fn total_from_row(row: &SqliteRow) -> Option<Money> {
    let amount: String = row.get("total_amount");
    let currency: String = row.get("total_currency");
    Money::from_parts(&currency, &amount)
        .inspect_err(|err| tracing::error!(%err, "skipping row with unusable total"))
        .ok()
}

pub(crate) fn details_columns(details: &Details) -> (&'static str, String) {
    let (kind, details) = match details {
        Details::Standard {
            title,
            account_number,
            recipient,
        } => (
            "standard",
            json!({"title": title, "account_number": account_number.as_str(), "recipient": recipient}),
        ),
        Details::Tax { form } => ("tax", json!({"form_symbol": tax_form_str(*form)})),
    };
    (kind, details.to_string())
}

pub(crate) fn details_from_columns(kind: &str, details: &str) -> Result<Details, String> {
    let details: Value = serde_json::from_str(details).map_err(|err| err.to_string())?;
    let field = |name: &str| {
        details[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{kind} details missing {name}"))
    };
    match kind {
        "standard" => {
            let account_number =
                AccountNumber::from(field("account_number")?).map_err(|err| err.to_string())?;
            Details::standard(field("title")?, account_number, field("recipient")?)
                .map_err(|err| err.to_string())
        }
        "tax" => match field("form_symbol")?.as_str() {
            "VAT-7" => Ok(Details::tax(TaxForm::Vat7)),
            "PPL" => Ok(Details::tax(TaxForm::Ppl)),
            other => Err(format!("unknown tax form {other}")),
        },
        other => Err(format!("unknown payment type {other}")),
    }
}

fn tax_form_str(form: TaxForm) -> &'static str {
    match form {
        TaxForm::Vat7 => "VAT-7",
        TaxForm::Ppl => "PPL",
    }
}
