mod payments;

use std::sync::Arc;

use axum::{Router, routing::get};

use crate::domain::payment::PaymentService;

#[derive(Clone)]
pub struct AppState {
    pub payment_service: Arc<PaymentService>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .merge(payments::router())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}
