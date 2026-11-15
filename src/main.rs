mod api;
mod app;
mod domain;
mod infra;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use api::AppState;
use app::email_watch_job::EmailWatchJob;
use app::event_bus::EventBus;
use app::outbox::OutboxRelayJob;
use app::payment;
use app::payment_schedule::PaymentScheduleService;
use app::payment_schedule_job::PaymentScheduleJob;
use app::settings::{Setting, SettingsService};
use app::startup_task::StartupTask;
use infra::Config;
use infra::discord::{DiscordConsumer, DiscordReactionListener};
use infra::event_bus::TokioEventBus;
use infra::secrets::Keyring;
use infra::tray::PendingAckWatch;

fn main() {
    infra::init();
    tracing::info!("starting up");
    infra::enter_home_dir();

    let config = Config::load();
    let server_port = config.server_port;
    let paused = Arc::new(AtomicBool::new(false));
    let pending_ack = Arc::new(AtomicBool::new(false));
    let tray_pending_ack = pending_ack.clone();

    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
            .block_on(serve(config, pending_ack));
    });

    // the tray needs the OS event loop on the main thread, so the server runs on its own thread.
    infra::tray::spawn(server_port, paused, tray_pending_ack);
}

async fn serve(config: Config, pending_ack: Arc<AtomicBool>) {
    let pool = infra::persistence::connect("sqlite:kittzo.db")
        .await
        .expect("failed to connect to database"); // intentional panic, db is a necessary dependency

    let settings = Arc::new(
        SettingsService::load(
            pool.clone(),
            Arc::new(Keyring),
            config.setting_defaults(),
            Setting::env_overrides(),
        )
        .await
        .expect("failed to load settings"),
    );
    let payment_service = Arc::new(payment::Service::new(pool.clone()));
    let schedule_service = Arc::new(PaymentScheduleService::new(pool.clone()));
    let bus: Arc<dyn EventBus> = Arc::new(TokioEventBus::new());

    // the discord consumer subscribes to the bus before the relay job can publish anything.
    let tasks: Vec<Arc<dyn StartupTask>> = vec![
        Arc::new(PaymentScheduleJob::new(
            schedule_service.clone(),
            config.payment_schedule(),
        )),
        Arc::new(PendingAckWatch::new(payment_service.clone(), pending_ack)),
        Arc::new(DiscordConsumer::new(
            settings.clone(),
            pool.clone(),
            bus.clone(),
        )),
        Arc::new(OutboxRelayJob::new(
            pool.clone(),
            bus.clone(),
            config.outbox(),
        )),
        Arc::new(DiscordReactionListener::new(
            settings.clone(),
            pool.clone(),
            payment_service.clone(),
        )),
        Arc::new(EmailWatchJob::new(
            settings.clone(),
            payment_service.clone(),
            config.openrouter.clone(),
            pool.clone(),
            config.email.watch(),
        )),
    ];

    for task in tasks {
        tokio::spawn(async move {
            tracing::info!(name = task.name(), "running startup task");
            task.run().await;
        });
    }

    let state = AppState {
        payment_service,
        schedule_service,
        settings,
    };

    let addr = format!("127.0.0.1:{}", config.server_port);
    api::serve(&addr, state).await.expect("server error");
}
