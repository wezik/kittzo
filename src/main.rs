mod api;
mod app;
mod domain;
mod infra;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use api::AppState;
use app::email;
use app::email_watch_job::EmailWatchJob;
use app::event_bus::EventBus;
use app::outbox::OutboxRelayJob;
use app::payment;
use app::payment_schedule::PaymentScheduleService;
use app::payment_schedule_job::PaymentScheduleJob;
use app::settings::{self, Setting, SettingsService};
use infra::Config;
use infra::config::OpenRouterConfig;
use infra::discord::{DiscordConsumer, DiscordReactionListener};
use infra::email::OpenRouterParser;
use infra::event_bus::TokioEventBus;
use infra::imap::GmailMailbox;
use infra::secrets::Keyring;
use infra::tray::PendingAckWatch;

fn main() {
    infra::init();
    tracing::info!("starting up");
    infra::enter_home_dir();

    let config = Config::load();
    let server_port = config.server_port;
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
    infra::tray::spawn(server_port, tray_pending_ack);
}

async fn serve(config: Config, pending_ack: Arc<AtomicBool>) {
    let pool = infra::persistence::connect("sqlite:kittzo.db")
        .await
        .expect("failed to connect to database"); // intentional panic, db is a necessary dependency

    let settings_service = Arc::new(SettingsService::new(
        pool.clone(),
        Arc::new(Keyring),
        HashMap::from([
            (Setting::ImapHost, config.email.imap_host.clone()),
            (Setting::ImapLabel, config.email.label.clone()),
            (Setting::OpenrouterModel, config.openrouter.model.clone()),
        ]),
        settings::env_overrides(),
    ));
    let settings = settings_service
        .resolve()
        .await
        .expect("failed to load settings");

    let payment_service = Arc::new(payment::Service::new(pool.clone()));
    let schedule_service = Arc::new(PaymentScheduleService::new(pool.clone()));

    tokio::spawn(
        PaymentScheduleJob::new(schedule_service.clone(), config.payment_schedule.clone()).run(),
    );
    tokio::spawn(PendingAckWatch::new(payment_service.clone(), pending_ack).run());

    let bus: Arc<dyn EventBus> = Arc::new(TokioEventBus::new());
    let missing = settings.missing(&Setting::DISCORD);
    if !missing.is_empty() {
        tracing::warn!(
            ?missing,
            "discord disabled, payment events will stay in the outbox"
        );
    } else {
        tracing::info!("discord enabled");
        // subscribes before the relay job below can publish anything.
        tokio::spawn(
            DiscordConsumer::new(
                settings.get(Setting::DiscordToken),
                HashMap::from([(
                    "confirmations".to_string(),
                    settings.get(Setting::DiscordChannel),
                )]),
                pool.clone(),
                bus.clone(),
            )
            .run(),
        );
        tokio::spawn(OutboxRelayJob::new(pool.clone(), bus.clone(), config.outbox.clone()).run());
        tokio::spawn(
            DiscordReactionListener::new(
                settings.get(Setting::DiscordToken),
                pool.clone(),
                payment_service.clone(),
            )
            .run(),
        );
    }

    let missing = settings.missing(&Setting::EMAIL);
    if !missing.is_empty() {
        tracing::warn!(?missing, "email watcher disabled");
    } else {
        tracing::info!("email watcher enabled");
        let parser = Arc::new(OpenRouterParser::new(OpenRouterConfig {
            api_key: settings.get(Setting::OpenrouterApiKey),
            model: settings.get(Setting::OpenrouterModel),
            ..config.openrouter.clone()
        }));
        let email_service = Arc::new(email::Service::new(parser, payment_service.clone()));
        let mailbox = Arc::new(GmailMailbox::new(
            settings.get(Setting::ImapHost),
            settings.get(Setting::ImapUser),
            settings.get(Setting::ImapAppPassword),
            settings.get(Setting::ImapLabel),
        ));
        tokio::spawn(
            EmailWatchJob::new(
                mailbox,
                email_service,
                pool.clone(),
                config.email.watch.clone(),
            )
            .run(),
        );
    }

    let state = AppState {
        payment_service,
        schedule_service,
        settings: settings_service,
    };

    let addr = format!("127.0.0.1:{}", config.server_port);
    api::serve(&addr, state).await.expect("server error");
}
