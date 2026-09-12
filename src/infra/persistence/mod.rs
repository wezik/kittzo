use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::domain::money::Money;

pub mod sqlx_confirmation_repository;
pub mod sqlx_payment_repository;
pub mod sqlx_payment_schedule_repository;

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
