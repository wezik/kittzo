use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::app::email::Service;
use crate::app::payment;
use crate::app::settings::SettingsService;
use crate::app::startup_task::StartupTask;
use crate::domain::email::Mailbox;
use crate::infra::config::OpenRouterConfig;
use crate::infra::email::OpenRouterParser;
use crate::infra::imap::GmailMailbox;
use crate::infra::persistence::sqlx_processed_email_repository as processed;

#[derive(Debug, Clone, PartialEq)]
pub struct EmailWatchConfig {
    pub poll_interval_secs: u64,
    pub max_attempts: u32,
}

impl Default for EmailWatchConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            max_attempts: 3,
        }
    }
}

pub struct EmailWatchJob {
    settings: Arc<SettingsService>,
    payments: Arc<payment::Service>,
    openrouter: OpenRouterConfig,
    pool: SqlitePool,
    config: EmailWatchConfig,
}

impl EmailWatchJob {
    pub fn new(
        settings: Arc<SettingsService>,
        payments: Arc<payment::Service>,
        openrouter: OpenRouterConfig,
        pool: SqlitePool,
        config: EmailWatchConfig,
    ) -> Self {
        Self {
            settings,
            payments,
            openrouter,
            pool,
            config,
        }
    }

    async fn tick(&self) {
        let Some(email) = self.settings.current().email() else {
            tracing::debug!("email not configured, skipping");
            return;
        };
        let mailbox = GmailMailbox::new(
            email.imap_host,
            email.imap_user,
            email.app_password,
            email.label,
        );
        let parser = Arc::new(OpenRouterParser::new(OpenRouterConfig {
            api_key: email.openrouter_api_key,
            model: email.openrouter_model,
            ..self.openrouter.clone()
        }));
        let service = Service::new(parser, self.payments.clone());
        self.process(&mailbox, &service).await;
    }

    async fn process(&self, mailbox: &dyn Mailbox, service: &Service) {
        let known = match processed::finished(&self.pool).await {
            Ok(known) => known,
            Err(err) => {
                tracing::error!(%err, "failed to load processed emails");
                return;
            }
        };
        let emails = match mailbox.fetch_new(&known).await {
            Ok(emails) => emails,
            Err(err) => {
                tracing::error!(%err, "failed to fetch emails");
                return;
            }
        };
        if emails.is_empty() {
            tracing::debug!("no new emails");
            return;
        }

        for email in emails {
            let recorded = match service.process(&email).await {
                Ok(created) => {
                    processed::mark_processed(&self.pool, &email.external_id, created).await
                }
                Err(err) => match processed::record_failure(
                    &self.pool,
                    &email.external_id,
                    &err.to_string(),
                    self.config.max_attempts,
                )
                .await
                {
                    Ok(attempts) if attempts >= self.config.max_attempts => {
                        tracing::error!(
                            %err,
                            message_id = %email.external_id,
                            subject = %email.subject,
                            attempts,
                            "giving up on email, delete its processed_emails row to retry"
                        );
                        Ok(())
                    }
                    Ok(attempts) => {
                        tracing::warn!(
                            %err,
                            message_id = %email.external_id,
                            subject = %email.subject,
                            attempts,
                            "failed to process email, will retry"
                        );
                        Ok(())
                    }
                    Err(store_err) => Err(store_err),
                },
            };
            if let Err(err) = recorded {
                tracing::error!(%err, message_id = %email.external_id, "failed to record email outcome");
            }
        }
    }
}

#[async_trait]
impl StartupTask for EmailWatchJob {
    fn name(&self) -> &str {
        "email-watch-job"
    }

    async fn run(&self) {
        loop {
            self.tick().await;
            tokio::time::sleep(Duration::from_secs(self.config.poll_interval_secs)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::app::settings::some_settings;
    use crate::domain::email::{self, Email, MailboxError, Parser, ParsingResult};
    use crate::domain::money::some_money;
    use crate::domain::payment::{Payment, Source, some_standard_details};
    use crate::infra::persistence::connect;
    use uuid::Uuid;

    struct FakeMailbox(Vec<Email>);

    #[async_trait]
    impl Mailbox for FakeMailbox {
        async fn fetch_new(&self, known: &HashSet<String>) -> Result<Vec<Email>, MailboxError> {
            Ok(self
                .0
                .iter()
                .filter(|email| !known.contains(&email.external_id))
                .cloned()
                .collect())
        }
    }

    struct FakeParser {
        calls: AtomicUsize,
        fail: Mutex<bool>,
    }

    #[async_trait]
    impl Parser for FakeParser {
        async fn parse(&self, email: &Email) -> Result<ParsingResult, email::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if *self.fail.lock().unwrap() {
                return Err(email::Error::Parse("model timed out".into()));
            }
            let source = Source::Email {
                message_id: email.external_id.clone(),
            };
            Ok(ParsingResult {
                results: vec![Ok(Payment::new(
                    some_money(),
                    some_standard_details(),
                    source,
                ))],
            })
        }
    }

    fn email(id: &str) -> Email {
        Email {
            id: Uuid::new_v4(),
            external_id: id.into(),
            from: "a@x.pl".into(),
            subject: "Faktura".into(),
            text: "do zapłaty".into(),
            pdfs: Vec::new(),
        }
    }

    struct Harness {
        job: EmailWatchJob,
        mailbox: FakeMailbox,
        service: Service,
    }

    impl Harness {
        async fn process(&self) {
            self.job.process(&self.mailbox, &self.service).await;
        }
    }

    async fn job(
        emails: Vec<Email>,
        fail: bool,
    ) -> (Harness, Arc<FakeParser>, Arc<payment::Service>) {
        let pool = connect("sqlite::memory:").await.unwrap();
        let payments = Arc::new(payment::Service::new(pool.clone()));
        let parser = Arc::new(FakeParser {
            calls: AtomicUsize::new(0),
            fail: Mutex::new(fail),
        });
        let job = EmailWatchJob::new(
            some_settings(pool.clone(), &[]).await,
            payments.clone(),
            OpenRouterConfig::default(),
            pool,
            EmailWatchConfig::default(),
        );
        let harness = Harness {
            job,
            mailbox: FakeMailbox(emails),
            service: Service::new(parser.clone(), payments.clone()),
        };
        (harness, parser, payments)
    }

    #[tokio::test]
    async fn each_email_is_processed_once() {
        let (harness, parser, payments) = job(vec![email("<a@x>"), email("<b@x>")], false).await;

        harness.process().await;
        harness.process().await;

        assert_eq!(parser.calls.load(Ordering::SeqCst), 2);
        assert_eq!(payments.list(None).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failing_email_is_retried_up_to_the_limit_then_left_alone() {
        let (harness, parser, payments) = job(vec![email("<a@x>")], true).await;

        for _ in 0..5 {
            harness.process().await;
        }

        assert_eq!(parser.calls.load(Ordering::SeqCst), 3);
        assert!(payments.list(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_email_that_recovers_before_the_limit_creates_its_payment() {
        let (harness, parser, payments) = job(vec![email("<a@x>")], true).await;
        harness.process().await;

        *parser.fail.lock().unwrap() = false;
        harness.process().await;
        harness.process().await;

        assert_eq!(parser.calls.load(Ordering::SeqCst), 2);
        assert_eq!(payments.list(None).await.unwrap().len(), 1);
    }
}
