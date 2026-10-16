use sqlx::{Row, Sqlite, SqliteConnection, sqlite::SqliteRow};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::payment::Details;
use crate::domain::payment_schedule::{PaymentSchedule, Recurrence};
use crate::domain::version::Version;
use crate::infra::persistence::{Error, unexpected};

use super::{details_columns, details_from_columns, total_from_row};

pub async fn create<'e, E>(
    executor: E,
    schedule: &PaymentSchedule,
) -> Result<PaymentSchedule, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let (kind, details) = details_columns(&schedule.next_details);
    let result = sqlx::query(
        "INSERT INTO payment_schedules
         (id, version, created_at, updated_at, total_amount, total_currency, title_template, type, details, recurrence_type, day_of_month, next_due_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(schedule.id.to_string())
    .bind(i64::from(schedule.version))
    .bind(schedule.created_at)
    .bind(schedule.updated_at)
    .bind(schedule.total.amount().to_string())
    .bind(schedule.total.currency_code())
    .bind(&schedule.title_template)
    .bind(kind)
    .bind(&details)
    .bind(recurrence_type(&schedule.recurrence))
    .bind(day_of_month(&schedule.recurrence) as i64)
    .bind(schedule.next_due_at)
    .execute(executor)
    .await
    .map_err(unexpected)?;

    if result.rows_affected() == 0 {
        return Err(Error::AlreadyExists);
    }

    tracing::info!(schedule_id = %schedule.id, "created payment schedule");
    Ok(schedule.clone())
}

pub async fn update(
    conn: &mut SqliteConnection,
    schedule: &PaymentSchedule,
) -> Result<PaymentSchedule, Error> {
    let next_version = schedule.version.next();
    let (kind, details) = details_columns(&schedule.next_details);

    let result = sqlx::query(
        "UPDATE payment_schedules
         SET version = ?, updated_at = ?, total_amount = ?, total_currency = ?, title_template = ?, type = ?, details = ?, recurrence_type = ?, day_of_month = ?, next_due_at = ?
         WHERE id = ? AND version = ?",
    )
    .bind(i64::from(next_version))
    .bind(schedule.updated_at)
    .bind(schedule.total.amount().to_string())
    .bind(schedule.total.currency_code())
    .bind(&schedule.title_template)
    .bind(kind)
    .bind(&details)
    .bind(recurrence_type(&schedule.recurrence))
    .bind(day_of_month(&schedule.recurrence) as i64)
    .bind(schedule.next_due_at)
    .bind(schedule.id.to_string())
    .bind(i64::from(schedule.version))
    .execute(&mut *conn)
    .await
    .map_err(unexpected)?;

    if result.rows_affected() == 0 {
        let exists = sqlx::query("SELECT 1 FROM payment_schedules WHERE id = ?")
            .bind(schedule.id.to_string())
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

    tracing::info!(schedule_id = %schedule.id, "updated payment schedule");
    Ok(PaymentSchedule {
        version: next_version,
        ..schedule.clone()
    })
}

pub async fn find_all<'e, E>(executor: E) -> Result<Vec<PaymentSchedule>, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    Ok(
        sqlx::query("SELECT * FROM payment_schedules ORDER BY created_at")
            .fetch_all(executor)
            .await
            .map_err(unexpected)?
            .iter()
            .filter_map(row_to_schedule)
            .collect(),
    )
}

pub async fn find_due<'e, E>(executor: E, today: Date) -> Result<Vec<PaymentSchedule>, Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    Ok(
        sqlx::query("SELECT * FROM payment_schedules WHERE next_due_at <= ? ORDER BY created_at")
            .bind(today)
            .fetch_all(executor)
            .await
            .map_err(unexpected)?
            .iter()
            .filter_map(row_to_schedule)
            .collect(),
    )
}

pub async fn delete<'e, E>(executor: E, id: Uuid) -> Result<(), Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let result = sqlx::query("DELETE FROM payment_schedules WHERE id = ?")
        .bind(id.to_string())
        .execute(executor)
        .await
        .map_err(unexpected)?;
    if result.rows_affected() == 0 {
        return Err(Error::NotFound);
    }
    tracing::info!(schedule_id = %id, "deleted payment schedule");
    Ok(())
}

fn recurrence_type(recurrence: &Recurrence) -> &'static str {
    match recurrence {
        Recurrence::Monthly { .. } => "monthly",
    }
}

fn day_of_month(recurrence: &Recurrence) -> u8 {
    match recurrence {
        Recurrence::Monthly { day_of_month } => *day_of_month,
    }
}

fn row_to_schedule(row: &SqliteRow) -> Option<PaymentSchedule> {
    let id: String = row.get("id");
    let version: i64 = row.get("version");
    let created_at: OffsetDateTime = row.get("created_at");
    let updated_at: OffsetDateTime = row.get("updated_at");
    let total = total_from_row(row)?;
    let title_template: String = row.get("title_template");
    let recurrence_type: String = row.get("recurrence_type");
    let day_of_month: i64 = row.get("day_of_month");
    let next_due_at: Date = row.get("next_due_at");
    let details: String = row.get("details");

    let kind: String = row.get("type");
    let next_details = details_from_columns(&kind, &details)
        .inspect_err(|err| tracing::error!(%err, %id, "skipping schedule with unusable details"))
        .ok()?;
    if !matches!(next_details, Details::Standard { .. }) {
        tracing::error!(%id, "skipping schedule with non-standard details");
        return None;
    }

    let recurrence = match recurrence_type.as_str() {
        "monthly" => Recurrence::Monthly {
            day_of_month: day_of_month as u8,
        },
        other => panic!("unknown recurrence_type in db: {other}"),
    };

    Some(PaymentSchedule {
        id: Uuid::parse_str(&id).expect("invalid payment schedule id uuid"),
        version: Version::from(version),
        created_at,
        updated_at,
        total,
        title_template,
        next_due_at,
        next_details,
        recurrence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::payment_schedule::some_schedule;
    use crate::infra::persistence::connect;
    use sqlx::SqlitePool;

    async fn pool() -> SqlitePool {
        connect("sqlite::memory:").await.unwrap()
    }

    fn today() -> Date {
        OffsetDateTime::now_utc().date()
    }

    fn schedule_due(next_due_at: Date) -> PaymentSchedule {
        PaymentSchedule {
            next_due_at,
            ..some_schedule("Czynsz {MM}/{YYYY}", 1)
        }
    }

    #[tokio::test]
    async fn create_round_trips() {
        let pool = pool().await;
        let schedule = schedule_due(today());

        let created = create(&pool, &schedule).await.unwrap();

        assert_eq!(created, schedule);
        assert_eq!(find_all(&pool).await.unwrap(), vec![schedule]);
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_id() {
        let pool = pool().await;
        let schedule = schedule_due(today());
        create(&pool, &schedule).await.unwrap();

        let err = create(&pool, &schedule).await.unwrap_err();

        assert!(matches!(err, Error::AlreadyExists));
    }

    #[tokio::test]
    async fn find_due_skips_schedules_not_yet_due() {
        let pool = pool().await;
        let due = create(&pool, &schedule_due(today())).await.unwrap();
        create(&pool, &schedule_due(today() + time::Duration::days(60)))
            .await
            .unwrap();

        assert_eq!(find_due(&pool, today()).await.unwrap(), vec![due]);
    }

    #[tokio::test]
    async fn update_persists_changes_and_bumps_version() {
        let pool = pool().await;
        let mut schedule = create(&pool, &schedule_due(today())).await.unwrap();
        schedule.produce(today()).unwrap();

        let updated = update(&mut pool.acquire().await.unwrap(), &schedule)
            .await
            .unwrap();

        assert_eq!(updated.version, schedule.version.next());
        assert_eq!(find_all(&pool).await.unwrap(), vec![updated]);
    }

    #[tokio::test]
    async fn update_rejects_a_stale_version() {
        let pool = pool().await;
        let schedule = create(&pool, &schedule_due(today())).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        update(&mut conn, &schedule).await.unwrap();

        let err = update(&mut conn, &schedule).await.unwrap_err();

        assert!(matches!(err, Error::ConcurrentUpdate));
    }

    #[tokio::test]
    async fn update_rejects_a_missing_schedule() {
        let pool = pool().await;
        let err = update(&mut pool.acquire().await.unwrap(), &schedule_due(today()))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test]
    async fn delete_removes_the_schedule() {
        let pool = pool().await;
        let schedule = create(&pool, &schedule_due(today())).await.unwrap();
        let other = create(&pool, &schedule_due(today())).await.unwrap();

        delete(&pool, schedule.id).await.unwrap();

        assert_eq!(find_all(&pool).await.unwrap(), vec![other]);
    }

    #[tokio::test]
    async fn delete_of_a_missing_schedule_is_not_found() {
        let pool = pool().await;
        let err = delete(&pool, Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }
}
