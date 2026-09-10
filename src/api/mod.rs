mod payments;

use std::sync::Arc;

use axum::{Router, routing::get};

use crate::domain::payment::PaymentService;

#[derive(Clone)]
pub struct AppState {
    pub payment_service: Arc<PaymentService>,
}

pub async fn serve(addr: &str, state: AppState) -> std::io::Result<()> {
    let router = Router::new()
        .route("/health", get(health))
        .merge(payments::router())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, router).await
}

async fn health() -> &'static str {
    "ok"
}
