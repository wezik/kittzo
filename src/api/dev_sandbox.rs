use axum::Router;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use serde::Deserialize;
use time::Date;
use time::format_description::well_known::Iso8601;

use crate::domain::payment;

use super::AppState;
use super::payments;

#[derive(Deserialize)]
struct CreatePaymentRequest {
    #[serde(flatten)]
    payment: payments::CreatePaymentRequest,
    source: payments::PaymentSourceDto,
}

impl TryFrom<payments::PaymentSourceDto> for payment::Source {
    type Error = String;

    fn try_from(value: payments::PaymentSourceDto) -> Result<Self, Self::Error> {
        Ok(match value {
            payments::PaymentSourceDto::Manual => Self::Manual,
            payments::PaymentSourceDto::Schedule {
                schedule_id,
                occurrence_date,
            } => Self::Schedule {
                schedule_id,
                occurrence_date: Date::parse(&occurrence_date, &Iso8601::DATE)
                    .map_err(|err| format!("invalid occurrence_date: {err}"))?,
            },
            payments::PaymentSourceDto::Email { message_id } => Self::Email { message_id },
        })
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/dev-sandbox/payments", post(post_create_payment))
}

async fn post_create_payment(
    State(state): State<AppState>,
    Json(body): Json<CreatePaymentRequest>,
) -> impl IntoResponse {
    let source = match payment::Source::try_from(body.source) {
        Ok(source) => source,
        Err(err) => return (StatusCode::BAD_REQUEST, err).into_response(),
    };
    match state
        .payment_service
        .create(
            body.payment.total.into(),
            body.payment.details.into(),
            source,
        )
        .await
    {
        Ok(payment) => (
            StatusCode::CREATED,
            Json(payments::PaymentResponse::from(payment)),
        )
            .into_response(),
        Err(err) => payments::map_err_to_response(err),
    }
}
