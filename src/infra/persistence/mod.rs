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
pub mod sqlx_settings_repository;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("already exists")]
    AlreadyExists,

    #[error("not found")]
    NotFound,

    #[error("updated concurrently")]
    ConcurrentUpdate,

    #[error("corrupt row: {0}")]
    Corrupt(String),

    #[error(transparent)]
    Unexpected(#[from] sqlx::Error),
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

pub(crate) fn details_from_columns(kind: &str, details: &str) -> Result<Details, Error> {
    let corrupt = |what: String| Error::Corrupt(format!("{kind} details: {what}"));
    let details: Value = serde_json::from_str(details).map_err(|err| corrupt(err.to_string()))?;
    let field = |name: &str| {
        details[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| corrupt(format!("missing {name}")))
    };
    match kind {
        "standard" => {
            let account_number = AccountNumber::from(field("account_number")?)
                .map_err(|err| corrupt(err.to_string()))?;
            Details::standard(field("title")?, account_number, field("recipient")?)
                .map_err(|err| corrupt(err.to_string()))
        }
        "tax" => match field("form_symbol")?.as_str() {
            "VAT-7" => Ok(Details::tax(TaxForm::Vat7)),
            "PPL" => Ok(Details::tax(TaxForm::Ppl)),
            other => Err(corrupt(format!("unknown tax form {other}"))),
        },
        other => Err(corrupt(format!("unknown type {other}"))),
    }
}

fn tax_form_str(form: TaxForm) -> &'static str {
    match form {
        TaxForm::Vat7 => "VAT-7",
        TaxForm::Ppl => "PPL",
    }
}
