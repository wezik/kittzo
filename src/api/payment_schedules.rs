use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{delete, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::serializers::details::DetailsDto;
use crate::api::serializers::money::MoneyDto;
use crate::domain::payment_schedule::{
    PaymentSchedule, PaymentScheduleError, PaymentScheduleStatus, Recurrence,
};

use super::AppState;
use super::serializers::rfc3339::OffsetDateTimeDto;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/payment-schedules", post(create).get(list))
        .route("/payment-schedules/{id}", delete(remove).put(edit))
}

#[derive(Deserialize)]
struct CreateScheduleRequest {
    total: MoneyDto,
    #[serde(flatten)]
    details: DetailsDto,
    recurrence: RecurrenceRequest,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecurrenceRequest {
    Monthly { day_of_month: u8 },
}

impl From<RecurrenceRequest> for Recurrence {
    fn from(recurrence: RecurrenceRequest) -> Self {
        match recurrence {
            RecurrenceRequest::Monthly { day_of_month } => Recurrence::Monthly { day_of_month },
        }
    }
}

#[derive(Serialize)]
struct PaymentScheduleResponse {
    id: Uuid,
    version: u32,
    created_at: OffsetDateTimeDto,
    updated_at: OffsetDateTimeDto,
    total: MoneyDto,
    #[serde(flatten)]
    details: DetailsDto,
    recurrence: RecurrenceResponse,
    next_due_at: String,
    status: PaymentScheduleStatusResponse,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecurrenceResponse {
    Monthly { day_of_month: u8 },
}

impl From<Recurrence> for RecurrenceResponse {
    fn from(recurrence: Recurrence) -> Self {
        match recurrence {
            Recurrence::Monthly { day_of_month } => RecurrenceResponse::Monthly { day_of_month },
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
            version: schedule.version.into(),
            created_at: schedule.created_at.into(),
            updated_at: schedule.updated_at.into(),
            total: schedule.total.into(),
            details: schedule.details.into(),
            recurrence: schedule.recurrence.into(),
            next_due_at: schedule.next_due_at.to_string(),
            status: schedule.status.into(),
        }
    }
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateScheduleRequest>,
) -> impl IntoResponse {
    match state
        .schedule_service
        .create(
            body.total.into(),
            body.details.into(),
            body.recurrence.into(),
        )
        .await
    {
        Ok(created) => (
            StatusCode::CREATED,
            Json(PaymentScheduleResponse::from(created)),
        )
            .into_response(),
        Err(PaymentScheduleError::AlreadyExists) => StatusCode::CONFLICT.into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to create payment schedule");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    let schedules = match state.schedule_service.find_all().await {
        Ok(schedules) => schedules,
        Err(err) => {
            tracing::error!(%err, "failed to list payment schedules");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let schedules: Vec<PaymentScheduleResponse> = schedules
        .into_iter()
        .map(PaymentScheduleResponse::from)
        .collect();
    Json(schedules).into_response()
}

async fn remove(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    match state.schedule_service.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(PaymentScheduleError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to delete payment schedule");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn edit(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateScheduleRequest>,
) -> impl IntoResponse {
    match state
        .schedule_service
        .edit(
            id,
            body.total.into(),
            body.details.into(),
            body.recurrence.into(),
        )
        .await
    {
        Ok(edited) => Json(PaymentScheduleResponse::from(edited)).into_response(),
        Err(PaymentScheduleError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(PaymentScheduleError::ConcurrentUpdate) => (
            StatusCode::CONFLICT,
            "schedule is being processed right now, try again",
        )
            .into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to edit payment schedule");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
