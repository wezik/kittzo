use std::collections::HashMap;

use sqlx::{Row, SqliteConnection, SqlitePool};

pub use super::Error;

pub async fn all(pool: &SqlitePool) -> Result<HashMap<String, String>, Error> {
    Ok(sqlx::query("SELECT key, value FROM settings")
        .fetch_all(pool)
        .await
        .map_err(Error::from)?
        .iter()
        .map(|row| (row.get("key"), row.get("value")))
        .collect())
}

pub async fn set(conn: &mut SqliteConnection, key: &str, value: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(Error::from)
}

pub async fn delete(conn: &mut SqliteConnection, key: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(Error::from)
}
