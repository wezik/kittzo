use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Sqlite, SqlitePool};

use crate::domain::money::Money;

// one connection avoids `SQLITE_BUSY` contention between writers.
pub async fn connect(url: &str) -> sqlx::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
}

impl sqlx::Type<Sqlite> for Money {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for Money {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<Sqlite>>::encode(self.to_string(), buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Money {
    fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<Sqlite>>::decode(value)?;
        Ok(s.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_money::iso;

    #[test]
    fn encode_decode_money_format_round_trips() {
        let money = Money::from_minor(1099, iso::USD);
        let wire: String = money.to_string();
        let decoded: Money = wire.parse().unwrap();
        assert_eq!(decoded, money);
    }
}
