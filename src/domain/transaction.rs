use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::infra::persistence::Error;

// DX friendly wrappers for sqlite transactions

pub async fn start_tx(pool: &SqlitePool) -> Result<Transaction<'static, Sqlite>, Error> {
    pool.begin().await.map_err(|sqlx_err| {
        tracing::error!(%sqlx_err, "failed to start transaction");
        Error
    })
}

pub async fn commit_tx(tx: Transaction<'static, Sqlite>) -> Result<(), Error> {
    tx.commit().await.map_err(|sqlx_err| {
        tracing::error!(%sqlx_err, "failed to commit transaction");
        Error
    })
}
