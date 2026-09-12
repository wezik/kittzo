use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::confirmation::{
    Confirmation, ConfirmationState, ConfirmationSubject, DecideError,
};

use super::AppState;
use super::serializers::rfc3339::OffsetDateTimeDto;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/confirmations", get(list_pending))
        .route("/confirmations/{id}/decide", post(decide))
}

#[derive(Deserialize)]
struct DecideRequest {
    approved: bool,
}

#[derive(Serialize)]
struct ConfirmationResponse {
    id: Uuid,
    version: u32,
    created_at: OffsetDateTimeDto,
    updated_at: OffsetDateTimeDto,
    subject: ConfirmationSubjectResponse,
    state: &'static str,
    decided_at: Option<OffsetDateTimeDto>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ConfirmationSubjectResponse {
    Payment { payment_id: Uuid },
}

impl From<Confirmation> for ConfirmationResponse {
    fn from(confirmation: Confirmation) -> Self {
        ConfirmationResponse {
            id: confirmation.id,
            version: confirmation.version.as_u32(),
            created_at: confirmation.created_at.into(),
            updated_at: confirmation.updated_at.into(),
            subject: match confirmation.subject {
                ConfirmationSubject::Payment(payment_id) => {
                    ConfirmationSubjectResponse::Payment { payment_id }
                }
            },
            state: match confirmation.state {
                ConfirmationState::Queued => "queued",
                ConfirmationState::Awaiting => "awaiting",
                ConfirmationState::Approved => "approved",
                ConfirmationState::Rejected => "rejected",
            },
            decided_at: confirmation.decided_at.map(Into::into),
        }
    }
}

async fn list_pending(State(state): State<AppState>) -> impl IntoResponse {
    let confirmations: Vec<ConfirmationResponse> = state
        .confirmation_service
        .find_pending()
        .await
        .into_iter()
        .map(ConfirmationResponse::from)
        .collect();
    Json(confirmations)
}

async fn decide(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<DecideRequest>,
) -> impl IntoResponse {
    match state.confirmation_service.decide(id, body.approved).await {
        Ok(confirmation) => Json(ConfirmationResponse::from(confirmation)).into_response(),
        Err(DecideError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(DecideError::AlreadyDecided) => StatusCode::CONFLICT.into_response(),
        Err(DecideError::ConcurrentUpdate) => StatusCode::CONFLICT.into_response(),
    }
}
