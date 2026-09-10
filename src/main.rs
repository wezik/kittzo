mod api;
mod domain;
mod infra;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use api::AppState;
use domain::payment::{PaymentRepository, PaymentService};
use infra::Config;
use infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;

fn main() {
    infra::init();
    tracing::info!("starting up");

    let config = Config::load();
    let server_port = config.server_port;
    let paused = Arc::new(AtomicBool::new(false));

    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
            .block_on(serve(config));
    });

    // the tray needs the OS event loop on the main thread, so the server runs on its own thread.
    infra::tray::spawn(server_port, paused);
}

async fn serve(config: Config) {
    let pool = infra::persistence::connect("sqlite:kittzo.db")
        .await
        .expect("failed to connect to database");
    let payment_repo: Arc<dyn PaymentRepository> = Arc::new(SqlitePaymentRepository::new(pool));
    let state = AppState {
        payment_service: Arc::new(PaymentService::new(payment_repo)),
    };

    let addr = format!("127.0.0.1:{}", config.server_port);
    api::serve(&addr, state).await.expect("server error");
}
