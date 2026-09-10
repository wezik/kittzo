mod api;
mod domain;
mod infra;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use api::AppState;
use domain::confirmation::{ConfirmationRepository, ConfirmationService};
use domain::payment::{PaymentRepository, PaymentService};
use domain::payment_schedule::{
    PaymentScheduleJob, PaymentScheduleRepository, PaymentScheduleService,
};
use domain::startup_task::StartupTask;
use infra::Config;
use infra::notifier::LoggingNotifier;
use infra::persistence::sqlx_confirmation_repository::SqliteConfirmationRepository;
use infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;
use infra::persistence::sqlx_payment_schedule_repository::SqlitePaymentScheduleRepository;

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
    let payment_repo: Arc<dyn PaymentRepository> =
        Arc::new(SqlitePaymentRepository::new(pool.clone()));
    let schedule_repo: Arc<dyn PaymentScheduleRepository> =
        Arc::new(SqlitePaymentScheduleRepository::new(pool.clone()));
    let confirmation_repo: Arc<dyn ConfirmationRepository> =
        Arc::new(SqliteConfirmationRepository::new(pool));

    let payment_service = Arc::new(PaymentService::new(payment_repo, Arc::new(LoggingNotifier)));
    let schedule_service = Arc::new(PaymentScheduleService::new(schedule_repo));
    let confirmation_service = Arc::new(ConfirmationService::new(
        confirmation_repo,
        Arc::new(LoggingNotifier),
    ));

    let tasks: Vec<Arc<dyn StartupTask>> = vec![Arc::new(PaymentScheduleJob::new(
        schedule_service.clone(),
        Arc::new(LoggingNotifier),
        config.payment_schedule(),
    ))];
    for task in tasks {
        tokio::spawn(async move {
            tracing::info!(name = task.name(), "running startup task");
            task.run().await;
        });
    }

    let state = AppState {
        payment_service,
        schedule_service,
        confirmation_service,
    };

    let addr = format!("127.0.0.1:{}", config.server_port);
    api::serve(&addr, state).await.expect("server error");
}
