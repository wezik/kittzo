mod api;
mod domain;
mod infra;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use api::AppState;
use domain::email::{self, EmailWatchJob};
use domain::event_bus::EventBus;
use domain::outbox::OutboxRelayJob;
use domain::payment;
use domain::payment_schedule::{
    PaymentScheduleJob, PaymentScheduleRepository, PaymentScheduleService,
};
use domain::startup_task::StartupTask;
use infra::Config;
use infra::discord::{DiscordConsumer, DiscordReactionListener};
use infra::event_bus::TokioEventBus;
use infra::imap::GmailMailbox;
use infra::openrouter::OpenRouterParser;
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
        .expect("failed to connect to database"); // intentional panic, db is a necessary dependency

    let schedule_repo: Arc<dyn PaymentScheduleRepository> =
        Arc::new(SqlitePaymentScheduleRepository::new(pool.clone()));

    let payment_service = Arc::new(payment::Service::new(pool.clone()));
    let schedule_service = Arc::new(PaymentScheduleService::new(schedule_repo));

    let mut tasks: Vec<Arc<dyn StartupTask>> = vec![Arc::new(PaymentScheduleJob::new(
        schedule_service.clone(),
        payment_service.clone(),
        config.payment_schedule(),
    ))];

    let bus: Arc<dyn EventBus> = Arc::new(TokioEventBus::new());
    if config.discord.token.is_empty() {
        tracing::warn!("no discord token configured, payment events will stay in the outbox");
    } else {
        // subscribes before the relay job below can publish anything.
        tasks.push(Arc::new(DiscordConsumer::new(
            config.discord.token.clone(),
            config.discord.channels.clone(),
            pool.clone(),
            bus.clone(),
        )));
        tasks.push(Arc::new(OutboxRelayJob::new(
            pool.clone(),
            bus.clone(),
            config.outbox(),
        )));
        tasks.push(Arc::new(DiscordReactionListener::new(
            config.discord.token.clone(),
            pool.clone(),
            payment_service.clone(),
        )));
    }

    let email_config = &config.email;
    if email_config.is_configured() {
        let parser = Arc::new(OpenRouterParser::new(
            email_config.openrouter_api_key.clone(),
            email_config.model.clone(),
        ));
        let email_service = Arc::new(email::Service::new(
            parser,
            payment_service.clone(),
            email_config.sender_rules(),
        ));
        let mailbox = Arc::new(GmailMailbox::new(
            email_config.imap_host.clone(),
            email_config.imap_user.clone(),
            email_config.app_password.clone(),
            email_config.label.clone(),
        ));
        tasks.push(Arc::new(EmailWatchJob::new(
            mailbox,
            email_service,
            pool.clone(),
            email_config.watch(),
        )));
    } else {
        tracing::warn!(
            "email watcher off: set [email] imap_user, app_password and openrouter_api_key to enable it"
        );
    }

    for task in tasks {
        tokio::spawn(async move {
            tracing::info!(name = task.name(), "running startup task");
            task.run().await;
        });
    }

    let state = AppState {
        payment_service,
        schedule_service,
    };

    let addr = format!("127.0.0.1:{}", config.server_port);
    api::serve(&addr, state).await.expect("server error");
}
