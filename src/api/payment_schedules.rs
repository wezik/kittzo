use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment_schedule::{PaymentSchedule, PaymentScheduleStatus, Recurrence};

use super::AppState;
use super::serializers::rfc3339::OffsetDateTimeDto;

pub fn router() -> Router<AppState> {
    Router::new().route("/payment-schedules", post(create).get(list))
}

#[derive(Deserialize)]
struct CreateScheduleRequest {
    total: Money,
    recurrence: RecurrenceRequest,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecurrenceRequest {
    EveryNMonths {
        interval_months: u32,
        day_of_month: u8,
    },
}

impl From<RecurrenceRequest> for Recurrence {
    fn from(recurrence: RecurrenceRequest) -> Self {
        match recurrence {
            RecurrenceRequest::EveryNMonths {
                interval_months,
                day_of_month,
            } => Recurrence::EveryNMonths {
                interval_months,
                day_of_month,
            },
        }
    }
}

#[derive(Serialize)]
struct PaymentScheduleResponse {
    id: Uuid,
    version: u32,
    created_at: OffsetDateTimeDto,
    updated_at: OffsetDateTimeDto,
    total: Money,
    recurrence: RecurrenceResponse,
    last_run_at: Option<OffsetDateTimeDto>,
    status: PaymentScheduleStatusResponse,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecurrenceResponse {
    EveryNMonths {
        interval_months: u32,
        day_of_month: u8,
    },
}

impl From<Recurrence> for RecurrenceResponse {
    fn from(recurrence: Recurrence) -> Self {
        match recurrence {
            Recurrence::EveryNMonths {
                interval_months,
                day_of_month,
            } => RecurrenceResponse::EveryNMonths {
                interval_months,
                day_of_month,
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum PaymentScheduleStatusResponse {
    Idle,
    Processing,
}

impl From<PaymentScheduleStatus> for PaymentScheduleStatusResponse {
    fn from(status: PaymentScheduleStatus) -> Self {
        match status {
            PaymentScheduleStatus::Idle => PaymentScheduleStatusResponse::Idle,
            PaymentScheduleStatus::Processing => PaymentScheduleStatusResponse::Processing,
        }
    }
}

impl From<PaymentSchedule> for PaymentScheduleResponse {
    fn from(schedule: PaymentSchedule) -> Self {
        PaymentScheduleResponse {
            id: schedule.id,
            version: schedule.version.as_u32(),
            created_at: schedule.created_at.into(),
            updated_at: schedule.updated_at.into(),
            total: schedule.total,
            recurrence: schedule.recurrence.into(),
            last_run_at: schedule.last_run_at.map(Into::into),
            status: schedule.status.into(),
        }
    }
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateScheduleRequest>,
) -> impl IntoResponse {
    let created = state
        .schedule_service
        .create(body.total, body.recurrence.into())
        .await;
    (StatusCode::CREATED, Json(PaymentScheduleResponse::from(created)))
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    let schedules: Vec<PaymentScheduleResponse> = state
        .schedule_service
        .find_all()
        .await
        .into_iter()
        .map(PaymentScheduleResponse::from)
        .collect();
    Json(schedules)
}
