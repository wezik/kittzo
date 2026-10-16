use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::serializers::details::DetailsDto;
use crate::api::serializers::money::MoneyDto;
use crate::api::serializers::version::VersionDto;
use crate::app;
use crate::domain::payment::{self, ListQueryAckFilter, ListQueryPaidFilter, Payment};

use super::AppState;
use super::serializers::rfc3339::OffsetDateTimeDto;

// Mappings

#[derive(Deserialize)]
pub(super) struct CreatePaymentRequest {
    pub(super) total: MoneyDto,
    #[serde(flatten)]
    pub(super) details: DetailsDto,
}

#[derive(Serialize)]
pub(super) struct PaymentResponse {
    id: Uuid,
    version: VersionDto,
    display_title: String,
    total: MoneyDto,
    #[serde(flatten)]
    details: DetailsDto,
    status: PaymentStatusResponse,
    ack: AckResponse,
    paid_at: Option<OffsetDateTimeDto>,
    created_at: OffsetDateTimeDto,
    updated_at: OffsetDateTimeDto,
    source: PaymentSourceDto,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum AckResponse {
    Pending,
    Accepted,
    Rejected,
}

impl From<payment::Ack> for AckResponse {
    fn from(ack: payment::Ack) -> Self {
        match ack {
            payment::Ack::Pending => AckResponse::Pending,
            payment::Ack::Confirmed { value: true } => AckResponse::Accepted,
            payment::Ack::Confirmed { value: false } => AckResponse::Rejected,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum PaymentStatusResponse {
    Unpaid,
    Paid,
}

impl From<Payment> for PaymentResponse {
    fn from(payment: Payment) -> Self {
        let (status, paid_at) = match payment.state {
            payment::State::Unpaid => (PaymentStatusResponse::Unpaid, None),
            payment::State::Paid { paid_at } => (PaymentStatusResponse::Paid, Some(paid_at.into())),
        };
        PaymentResponse {
            id: payment.id,
            version: payment.version.into(),
            display_title: payment.details.display_title(),
            total: payment.total.into(),
            details: payment.details.into(),
            status,
            ack: payment.ack.into(),
            paid_at,
            created_at: payment.created_at.into(),
            updated_at: payment.updated_at.into(),
            source: payment.source.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub(super) enum PaymentSourceDto {
    Manual,
    Schedule {
        schedule_id: Uuid,
        occurrence_date: String,
    },
    Email {
        message_id: String,
    },
}

impl From<payment::Source> for PaymentSourceDto {
    fn from(value: payment::Source) -> Self {
        match value {
            payment::Source::Manual => Self::Manual,
            payment::Source::Schedule {
                schedule_id,
                occurrence_date,
            } => Self::Schedule {
                schedule_id,
                occurrence_date: occurrence_date.to_string(),
            },
            payment::Source::Email { message_id } => Self::Email { message_id },
        }
    }
}

pub(super) fn map_err_to_response(err: app::payment::Error) -> Response {
    use app::payment::Error;
    match err {
        Error::AlreadyExists => StatusCode::CONFLICT.into_response(),
        Error::NotFound => StatusCode::NOT_FOUND.into_response(),
        Error::Domain(payment::Error::InvalidAccountNumber | payment::Error::InvalidRecipient) => {
            (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()).into_response()
        }
        Error::Domain(payment::Error::InvalidTransition { .. }) | Error::ConcurrentUpdate => {
            (StatusCode::CONFLICT, err.to_string()).into_response()
        }
        // NOTE: For now map those directly
        Error::Persistence => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    paid: Option<bool>,
    ack: Option<String>,
}

fn parse_ack(raw: &str) -> Result<ListQueryAckFilter, String> {
    match raw {
        "pending" => Ok(ListQueryAckFilter::Pending),
        "accepted" => Ok(ListQueryAckFilter::Accepted),
        other => Err(format!("unknown ack value: {other}")),
    }
}

// Routes

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/payments", get(get_list).post(post_create))
        .route("/payments/{id}/pay", post(post_pay))
        .route("/payments/{id}/ack", post(post_ack))
}

async fn post_create(
    State(state): State<AppState>,
    Json(body): Json<CreatePaymentRequest>,
) -> impl IntoResponse {
    match state
        .payment_service
        .create(
            body.total.into(),
            body.details.into(),
            payment::Source::Manual,
        )
        .await
    {
        Ok(payment) => (StatusCode::CREATED, Json(PaymentResponse::from(payment))).into_response(),
        Err(err) => map_err_to_response(err),
    }
}

async fn get_list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let ack_filter = match query.ack.as_deref().map(parse_ack).transpose() {
        Ok(ack_filter) => ack_filter,
        Err(err) => return (StatusCode::BAD_REQUEST, err).into_response(),
    };
    let paid_filter = query.paid.map(|paid| {
        if paid {
            ListQueryPaidFilter::Paid
        } else {
            ListQueryPaidFilter::Unpaid
        }
    });
    match state
        .payment_service
        .list(Some(payment::ListQuery {
            paid_filter,
            ack_filter,
        }))
        .await
    {
        Ok(payments) => Json(
            payments
                .into_iter()
                .map(PaymentResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => map_err_to_response(err),
    }
}

#[derive(Deserialize)]
struct AckRequest {
    accept: bool,
}

async fn post_pay(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    transition_response(state.payment_service.pay(id).await)
}

async fn post_ack(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AckRequest>,
) -> impl IntoResponse {
    transition_response(state.payment_service.ack(id, body.accept).await)
}

fn transition_response(result: Result<Payment, app::payment::Error>) -> Response {
    match result {
        Ok(payment) => Json(PaymentResponse::from(payment)).into_response(),
        Err(err) => map_err_to_response(err),
    }
}
