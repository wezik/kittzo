use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::infra::persistence::Error;

pub async fn start_tx(pool: &SqlitePool) -> Result<Transaction<'static, Sqlite>, Error> {
    Ok(pool.begin().await?)
}

pub async fn commit_tx(tx: Transaction<'static, Sqlite>) -> Result<(), Error> {
    Ok(tx.commit().await?)
}
