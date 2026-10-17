use sqlx::{Sqlite, SqliteConnection};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment::{
    Ack, ListQuery, ListQueryAckFilter, ListQueryPaidFilter, Payment, Source, State,
};
use crate::domain::version::Version;

use crate::infra::persistence::{Error, details_columns, details_from_columns, unexpected};

// sqlite's `Uuid` decode expects a 16-byte BLOB, but ids are stored as TEXT (`.to_string()`),
// so `id`/`schedule_id` are decoded as `String` here and parsed by hand in `row_to_payment`.
#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: String,
    version: i64,
    total_amount: String,
    total_currency: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    r#type: String,
    details: String,
    status: String,
    paid_at: Option<OffsetDateTime>,
    source_type: String,
    schedule_id: Option<String>,
    occurrence_date: Option<Date>,
    email_message_id: Option<String>,
    ack: String,
}

/// Takes any executor (`&pool` for a standalone write, `&mut *tx` to join a transaction a
/// caller already opened, e.g. alongside `sqlx_outbox_repository::create`).
pub async fn create<'e, E>(executor: E, payment: &Payment) -> Result<Payment, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let (source_name, schedule_id, occurrence_date, email_message_id) = match &payment.source {
        Source::Manual => ("manual", None, None, None),
        Source::Schedule {
            schedule_id,
            occurrence_date,
        } => (
            "schedule",
            Some(schedule_id.to_string()),
            Some(*occurrence_date),
            None,
        ),
        Source::Email { message_id } => ("email", None, None, Some(message_id.as_str())),
    };

    let (kind, details) = details_columns(&payment.details);

    let row = sqlx::query_as::<_, PaymentRow>(
        "
        INSERT INTO payments (
            id,
            version,
            total_amount,
            total_currency,
            created_at,
            updated_at,
            type,
            details,
            status,
            source_type,
            schedule_id,
            occurrence_date,
            email_message_id,
            ack
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'unpaid', ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO NOTHING
        RETURNING *
        ",
    )
    .bind(payment.id.to_string())
    .bind(i64::from(Version::FIRST))
    .bind(payment.total.amount().to_string())
    .bind(payment.total.currency_code())
    .bind(payment.created_at)
    .bind(payment.updated_at)
    .bind(kind)
    .bind(details)
    .bind(source_name)
    .bind(schedule_id)
    .bind(occurrence_date)
    .bind(email_message_id)
    .bind(ack_str(&payment.ack))
    .fetch_optional(executor)
    .await
    .map_err(unexpected)?
    .ok_or(Error::AlreadyExists)?;

    let payment = row_to_payment(row)?;
    tracing::info!(id = %payment.id, "created payment");
    Ok(payment)
}

pub async fn update(conn: &mut SqliteConnection, payment: &Payment) -> Result<Payment, Error> {
    let (status, paid_at) = state_columns(&payment.state);
    let next_version = payment.version.next();

    let result = sqlx::query(
        "UPDATE payments
         SET version = ?, updated_at = ?, status = ?, paid_at = ?, ack = ?
         WHERE id = ? AND version = ?",
    )
    .bind(i64::from(next_version))
    .bind(payment.updated_at)
    .bind(status)
    .bind(paid_at)
    .bind(ack_str(&payment.ack))
    .bind(payment.id.to_string())
    .bind(i64::from(payment.version))
    .execute(&mut *conn)
    .await
    .map_err(unexpected)?;

    if result.rows_affected() == 0 {
        let exists = sqlx::query("SELECT 1 FROM payments WHERE id = ?")
            .bind(payment.id.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(unexpected)?
            .is_some();

        return Err(if exists {
            Error::ConcurrentUpdate
        } else {
            Error::NotFound
        });
    }
    tracing::info!(id = %payment.id, status, "updated payment");

    Ok(Payment {
        version: next_version,
        ..payment.clone()
    })
}

pub async fn has_pending_ack<'e, E>(executor: E) -> Result<bool, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payments WHERE ack = 'pending')")
        .fetch_one(executor)
        .await
        .map_err(unexpected)
}

pub async fn find_all<'e, E>(executor: E, query: Option<ListQuery>) -> Result<Vec<Payment>, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut sql = String::from("SELECT * FROM payments");
    if let Some(ListQuery {
        paid_filter,
        ack_filter,
    }) = query
    {
        let filters: Vec<&str> = [
            paid_filter.map(|filter| match filter {
                ListQueryPaidFilter::Paid => "status = 'paid'",
                ListQueryPaidFilter::Unpaid => "status = 'unpaid'",
            }),
            ack_filter.map(|filter| match filter {
                ListQueryAckFilter::Pending => "ack = 'pending'",
                ListQueryAckFilter::Accepted => "ack = 'accepted'",
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !filters.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&filters.join(" AND "));
        }
    }
    sql.push_str(" ORDER BY created_at");

    sqlx::query_as::<_, PaymentRow>(sqlx::AssertSqlSafe(sql))
        .fetch_all(executor)
        .await
        .map_err(unexpected)?
        .into_iter()
        .map(row_to_payment)
        .collect()
}

pub async fn find<'e, E>(executor: E, id: Uuid) -> Result<Payment, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, PaymentRow>("SELECT * FROM payments WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(executor)
        .await
        .map_err(unexpected)?
        .ok_or(Error::NotFound)?;

    row_to_payment(row)
}

fn ack_str(ack: &Ack) -> &'static str {
    match ack {
        Ack::Pending => "pending",
        Ack::Confirmed { value: true } => "accepted",
        Ack::Confirmed { value: false } => "rejected",
    }
}

fn state_columns(state: &State) -> (&'static str, Option<OffsetDateTime>) {
    match state {
        State::Unpaid => ("unpaid", None),
        State::Paid { paid_at } => ("paid", Some(*paid_at)),
    }
}

fn row_to_payment(row: PaymentRow) -> Result<Payment, Error> {
    let id = Uuid::parse_str(&row.id).map_err(|err| {
        tracing::error!(%err, id = %row.id, "invalid payment id uuid");
        Error::Unexpected
    })?;
    let version = Version::from(row.version);

    let amount = row.total_amount;
    let currency = row.total_currency;

    let total = Money::from_parts(&currency, &amount).map_err(|err| {
        tracing::error!(%err, %amount, %currency, "failed to deserialize money type");
        Error::Unexpected
    })?;

    let source = (match row.source_type.as_str() {
        "manual" => Ok(Source::Manual),
        "schedule" => {
            let schedule_id = row.schedule_id.ok_or_else(|| {
                tracing::error!(
                    payment_id = %id,
                    source_type = %row.source_type,
                    "payment has schedule source type but schedule_id is missing"
                );
                Error::Unexpected
            })?;
            let schedule_id = Uuid::parse_str(&schedule_id).map_err(|err| {
                tracing::error!(%err, payment_id = %id, "invalid schedule id uuid");
                Error::Unexpected
            })?;

            let occurrence_date = row.occurrence_date.ok_or_else(|| {
                tracing::error!(
                    payment_id = %id,
                    source_type = %row.source_type,
                    "payment has schedule source type but occurence_date is missing"
                );
                Error::Unexpected
            })?;

            Ok(Source::Schedule {
                schedule_id,
                occurrence_date,
            })
        }

        "email" => row.email_message_id.map(|message_id| Source::Email { message_id }).ok_or_else(|| {
            tracing::error!(payment_id = %id, "payment has email source type but email_message_id is missing");
            Error::Unexpected
        }),

        _ => {
            tracing::error!(
                payment_id = %id,
                source_type = %row.source_type,
                "unrecognized source type"
            );
            Err(Error::Unexpected)
        }
    })?;

    let details = details_from_columns(&row.r#type, &row.details).map_err(|err| {
        tracing::error!(%err, payment_id = %id, "invalid payment details");
        Error::Unexpected
    })?;

    let state = match (row.status.as_str(), row.paid_at) {
        ("unpaid", _) => State::Unpaid,
        ("paid", Some(paid_at)) => State::Paid { paid_at },
        (other, paid_at) => {
            tracing::error!(payment_id = %id, status = other, ?paid_at, "unrecognized payment status");
            return Err(Error::Unexpected);
        }
    };

    let ack = match row.ack.as_str() {
        "pending" => Ack::Pending,
        "accepted" => Ack::Confirmed { value: true },
        "rejected" => Ack::Confirmed { value: false },
        other => {
            tracing::error!(payment_id = %id, ack = other, "unrecognized payment ack");
            return Err(Error::Unexpected);
        }
    };

    Ok(Payment {
        id,
        version,
        created_at: row.created_at,
        updated_at: row.updated_at,
        total,
        details,
        source,
        state,
        ack,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::some_money;
    use crate::domain::payment::{Details, TaxForm};
    use crate::domain::payment::{some_payment, some_payment_source_email, some_standard_details};
    use crate::infra::persistence::connect;
    use sqlx::SqlitePool;

    async fn pool() -> SqlitePool {
        connect("sqlite::memory:").await.unwrap()
    }

    fn manual_payment() -> Payment {
        some_payment()
    }

    #[tokio::test]
    async fn create_returns_the_created_payment() {
        // given
        let pool = pool().await;
        let payment = manual_payment();

        // when
        let created = create(&pool, &payment).await.unwrap();

        // then
        assert_eq!(created, payment);
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_id() {
        // given
        let pool = pool().await;
        let payment = manual_payment();
        create(&pool, &payment).await.unwrap();

        // when
        let err = create(&pool, &payment).await.unwrap_err();

        // then
        assert!(matches!(err, Error::AlreadyExists));
    }

    #[tokio::test]
    async fn find_all_returns_every_payment() {
        // given
        let pool = pool().await;
        let a = manual_payment();
        let b = manual_payment();
        create(&pool, &a).await.unwrap();
        create(&pool, &b).await.unwrap();

        // when
        let all = find_all(&pool, None).await.unwrap();

        // then
        assert_eq!(all.len(), 2);
        assert!(all.contains(&a));
        assert!(all.contains(&b));
    }

    #[tokio::test]
    async fn find_all_unpaid_skips_paid_payments() {
        // given
        let pool = pool().await;
        let unpaid = manual_payment();
        let mut paid = manual_payment();
        create(&pool, &unpaid).await.unwrap();
        create(&pool, &paid).await.unwrap();
        paid.pay().unwrap();
        update(&mut pool.acquire().await.unwrap(), &paid)
            .await
            .unwrap();

        // when
        let found = find_all(
            &pool,
            Some(ListQuery {
                paid_filter: Some(ListQueryPaidFilter::Unpaid),
                ack_filter: None,
            }),
        )
        .await
        .unwrap();

        // then
        assert_eq!(found, vec![unpaid]);
    }

    #[tokio::test]
    async fn find_all_combines_status_and_ack_filters() {
        // given
        let pool = pool().await;
        let accepted = manual_payment();
        let (pending, _) = Payment::new(
            some_money(),
            some_standard_details(),
            some_payment_source_email(),
        );
        create(&pool, &accepted).await.unwrap();
        create(&pool, &pending).await.unwrap();

        // when
        let found = find_all(
            &pool,
            Some(ListQuery {
                paid_filter: Some(ListQueryPaidFilter::Unpaid),
                ack_filter: Some(ListQueryAckFilter::Accepted),
            }),
        )
        .await
        .unwrap();

        // then
        assert_eq!(found, vec![accepted]);
    }

    #[tokio::test]
    async fn has_pending_ack_only_when_a_payment_awaits_ack() {
        // given
        let pool = pool().await;
        create(&pool, &manual_payment()).await.unwrap();
        assert!(!has_pending_ack(&pool).await.unwrap());
        let (pending, _) = Payment::new(
            some_money(),
            some_standard_details(),
            some_payment_source_email(),
        );

        // when
        create(&pool, &pending).await.unwrap();

        // then
        assert!(has_pending_ack(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn find_returns_the_matching_payment() {
        // given
        let pool = pool().await;
        let payment = manual_payment();
        create(&pool, &payment).await.unwrap();

        // when
        let found = find(&pool, payment.id).await.unwrap();

        // then
        assert_eq!(found, payment);
    }

    #[tokio::test]
    async fn email_source_and_tax_details_round_trip() {
        // given
        let pool = pool().await;
        let (payment, _) = Payment::new(
            Money::from_parts("PLN", "5.00").unwrap(),
            Details::tax(TaxForm::Vat7),
            Source::Email {
                message_id: "<abc@mail.gmail.com>".into(),
            },
        );

        // when
        create(&pool, &payment).await.unwrap();

        // then
        assert_eq!(find(&pool, payment.id).await.unwrap(), payment);
    }

    #[tokio::test]
    async fn find_missing_returns_not_found() {
        let pool = pool().await;
        let err = find(&pool, Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test]
    async fn paid_round_trips_and_bumps_version() {
        // given
        let pool = pool().await;
        let mut payment = manual_payment();
        create(&pool, &payment).await.unwrap();
        payment.pay().unwrap();

        // when
        let paid = update(&mut pool.acquire().await.unwrap(), &payment)
            .await
            .unwrap();

        // then
        let stored = find(&pool, payment.id).await.unwrap();
        assert_eq!(stored, paid);
        assert_eq!(stored.version, Version::FIRST.next());
        assert!(matches!(stored.state, State::Paid { .. }));
    }

    #[tokio::test]
    async fn ack_round_trips() {
        // given
        let pool = pool().await;
        let (mut payment, _) = Payment::new(
            some_money(),
            some_standard_details(),
            some_payment_source_email(),
        );
        create(&pool, &payment).await.unwrap();
        assert_eq!(find(&pool, payment.id).await.unwrap().ack, Ack::Pending);
        payment.ack(false).unwrap();

        // when
        update(&mut pool.acquire().await.unwrap(), &payment)
            .await
            .unwrap();

        // then
        assert_eq!(
            find(&pool, payment.id).await.unwrap().ack,
            Ack::Confirmed { value: false }
        );
    }

    #[tokio::test]
    async fn update_rejects_a_stale_version() {
        // given
        let pool = pool().await;
        let payment = manual_payment();
        create(&pool, &payment).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        update(&mut conn, &payment).await.unwrap();

        // when
        let err = update(&mut conn, &payment).await.unwrap_err();

        // then
        assert!(matches!(err, Error::ConcurrentUpdate));
    }

    #[tokio::test]
    async fn update_rejects_a_missing_payment() {
        let pool = pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let err = update(&mut conn, &manual_payment()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }
}
