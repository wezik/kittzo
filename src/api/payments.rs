use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::confirmation::ConfirmationState;
use crate::domain::money::Money;
use crate::domain::payment::{CreatePaymentCommand, Payment, PaymentError, PaymentSource};

use super::AppState;
use super::serializers::rfc3339::OffsetDateTimeDto;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ingest", post(ingest))
        .route("/payments", get(list))
        .route("/payments/{id}", get(find_by_id))
}

#[derive(Deserialize)]
struct IngestRequest {
    total: Money,
}

#[derive(Serialize)]
struct PaymentResponse {
    id: Uuid,
    version: u32,
    total: Money,
    created_at: OffsetDateTimeDto,
    source: PaymentSourceResponse,
    confirmation_state: &'static str,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaymentSourceResponse {
    Manual,
    Schedule {
        schedule_id: Uuid,
        occurrence_date: String,
    },
}

async fn to_response(state: &AppState, payment: Payment) -> PaymentResponse {
    let confirmation_state = state
        .confirmation_service
        .find_by_payment_id(payment.id)
        .await
        .map(|c| match c.state {
            ConfirmationState::Queued => "queued",
            ConfirmationState::Awaiting => "awaiting",
            ConfirmationState::Approved => "approved",
            ConfirmationState::Rejected => "rejected",
        })
        .unwrap_or("unknown");

    PaymentResponse {
        id: payment.id,
        version: payment.version.as_u32(),
        total: payment.total,
        created_at: payment.created_at.into(),
        source: payment.source.into(),
        confirmation_state,
    }
}

impl From<PaymentSource> for PaymentSourceResponse {
    fn from(source: PaymentSource) -> Self {
        match source {
            PaymentSource::Manual => PaymentSourceResponse::Manual,
            PaymentSource::Schedule {
                schedule_id,
                occurrence_date,
            } => PaymentSourceResponse::Schedule {
                schedule_id,
                occurrence_date: occurrence_date.to_string(),
            },
        }
    }
}

async fn ingest(
    State(state): State<AppState>,
    Json(body): Json<IngestRequest>,
) -> impl IntoResponse {
    let command = CreatePaymentCommand {
        total: body.total,
        source: PaymentSource::Manual,
        auto_ack: true,
    };

    match state.payment_service.create(command).await {
        Ok(payment) => {
            let response = to_response(&state, payment).await;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(PaymentError::AlreadyExists) => StatusCode::CONFLICT.into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to create payment");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    let payments = match state.payment_service.find_all().await {
        Ok(payments) => payments,
        Err(err) => {
            tracing::error!(%err, "failed to list payments");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut responses = Vec::new();
    for payment in payments {
        responses.push(to_response(&state, payment).await);
    }
    Json(responses).into_response()
}

async fn find_by_id(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    match state.payment_service.find_by_id(id).await {
        Ok(Some(payment)) => Json(to_response(&state, payment).await).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to find payment");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
