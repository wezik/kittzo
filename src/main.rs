mod api;
mod domain;
mod infra;

use std::sync::Arc;

use domain::payment::PaymentRepository;
use infra::Config;
use infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;

#[tokio::main]
async fn main() {
    infra::init();
    tracing::info!("starting up");

    let config = Config::load();

    let pool = infra::persistence::connect("sqlite:kittzo.db")
        .await
        .expect("failed to connect to database");
    let _payment_repo: Arc<dyn PaymentRepository> = Arc::new(SqlitePaymentRepository::new(pool));

    let addr = format!("127.0.0.1:{}", config.server_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind server address");
    tracing::info!(%addr, "listening");

    axum::serve(listener, api::router())
        .await
        .expect("server error");
}
