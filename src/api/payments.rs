use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment::{Payment, PaymentSource};

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

impl From<Payment> for PaymentResponse {
    fn from(payment: Payment) -> Self {
        PaymentResponse {
            id: payment.id,
            version: payment.version.as_u32(),
            total: payment.total,
            created_at: payment.created_at.into(),
            source: payment.source.into(),
        }
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
    let payment = state
        .payment_service
        .create(body.total, PaymentSource::Manual)
        .await;
    (StatusCode::CREATED, Json(PaymentResponse::from(payment)))
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    let payments: Vec<PaymentResponse> = state
        .payment_service
        .find_all()
        .await
        .into_iter()
        .map(PaymentResponse::from)
        .collect();
    Json(payments)
}

async fn find_by_id(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    match state.payment_service.find_by_id(id).await {
        Some(payment) => Json(PaymentResponse::from(payment)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
