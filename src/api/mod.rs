mod confirmations;
mod payment_schedules;
mod payments;
mod serializers;

use std::sync::Arc;

use axum::{Router, routing::get};
use tower_http::services::ServeDir;

use crate::domain::confirmation::ConfirmationService;
use crate::domain::payment::PaymentService;
use crate::domain::payment_schedule::PaymentScheduleService;

#[derive(Clone)]
pub struct AppState {
    pub payment_service: Arc<PaymentService>,
    pub schedule_service: Arc<PaymentScheduleService>,
    pub confirmation_service: Arc<ConfirmationService>,
}

pub async fn serve(addr: &str, state: AppState) -> std::io::Result<()> {
    let router = Router::new()
        .route("/health", get(health))
        .merge(payments::router())
        .merge(payment_schedules::router())
        .merge(confirmations::router())
        .with_state(state)
        .fallback_service(ServeDir::new("resources/static"));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, router).await
}

async fn health() -> &'static str {
    "ok"
}
