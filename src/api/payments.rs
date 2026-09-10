use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment::{Payment, PaymentSource};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ingest", post(ingest))
        .route("/payments", get(list))
        .route("/payments/{id}", get(find_by_id))
}

#[derive(Deserialize)]
struct IngestRequest {
    amount: Money,
}

#[derive(Serialize)]
struct PaymentResponse {
    id: Uuid,
    version: u32,
    amount: Money,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    source: PaymentSourceResponse,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaymentSourceResponse {
    Manual,
}

impl From<Payment> for PaymentResponse {
    fn from(payment: Payment) -> Self {
        PaymentResponse {
            id: payment.id,
            version: payment.version.as_u32(),
            amount: payment.amount,
            created_at: payment.created_at,
            source: payment.source.into(),
        }
    }
}

impl From<PaymentSource> for PaymentSourceResponse {
    fn from(source: PaymentSource) -> Self {
        match source {
            PaymentSource::Manual => PaymentSourceResponse::Manual,
        }
    }
}

async fn ingest(
    State(state): State<AppState>,
    Json(body): Json<IngestRequest>,
) -> impl IntoResponse {
    let payment = state.payment_service.ingest(body.amount).await;
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
