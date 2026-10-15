use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::domain::startup_task::StartupTask;
use crate::infra::persistence::sqlx_processed_email_repository as processed;

use super::{Mailbox, Service};

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
    mailbox: Arc<dyn Mailbox>,
    service: Arc<Service>,
    pool: SqlitePool,
    config: EmailWatchConfig,
}

impl EmailWatchJob {
    pub fn new(
        mailbox: Arc<dyn Mailbox>,
        service: Arc<Service>,
        pool: SqlitePool,
        config: EmailWatchConfig,
    ) -> Self {
        Self {
            mailbox,
            service,
            pool,
            config,
        }
    }

    async fn process(&self) {
        let known = match processed::finished(&self.pool).await {
            Ok(known) => known,
            Err(err) => {
                tracing::error!(%err, "failed to load processed emails");
                return;
            }
        };
        let emails = match self.mailbox.fetch_new(&known).await {
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
            let recorded = match self.service.process(&email).await {
                Ok(created) => {
                    processed::mark_processed(&self.pool, &email.message_id, created).await
                }
                Err(err) => match processed::record_failure(
                    &self.pool,
                    &email.message_id,
                    &err.to_string(),
                    self.config.max_attempts,
                )
                .await
                {
                    Ok(attempts) if attempts >= self.config.max_attempts => {
                        tracing::error!(
                            %err,
                            message_id = %email.message_id,
                            subject = %email.subject,
                            attempts,
                            "giving up on email, delete its processed_emails row to retry"
                        );
                        Ok(())
                    }
                    Ok(attempts) => {
                        tracing::warn!(
                            %err,
                            message_id = %email.message_id,
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
                tracing::error!(%err, message_id = %email.message_id, "failed to record email outcome");
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
            self.process().await;
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
    use crate::domain::email::{
        Candidate, CandidateKind, Email, EmailParser, MailboxError, ParseError,
    };
    use crate::domain::payment;
    use crate::infra::persistence::connect;

    struct FakeMailbox(Vec<Email>);

    #[async_trait]
    impl Mailbox for FakeMailbox {
        async fn fetch_new(&self, known: &HashSet<String>) -> Result<Vec<Email>, MailboxError> {
            Ok(self
                .0
                .iter()
                .filter(|email| !known.contains(&email.message_id))
                .cloned()
                .collect())
        }
    }

    struct FakeParser {
        calls: AtomicUsize,
        fail: Mutex<bool>,
    }

    #[async_trait]
    impl EmailParser for FakeParser {
        async fn parse(&self, _email: &Email) -> Result<Vec<Candidate>, ParseError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if *self.fail.lock().unwrap() {
                return Err(ParseError("model timed out".into()));
            }
            Ok(vec![Candidate {
                title: Some("Faktura 1".into()),
                amount: "10.00".into(),
                currency: "PLN".into(),
                invoice_nr: None,
                period: None,
                kind: CandidateKind::Standard {
                    account_number: "61109010140000071219812874".into(),
                    recipient: "X".into(),
                },
            }])
        }
    }

    fn email(id: &str) -> Email {
        Email {
            message_id: id.into(),
            from: "a@x.pl".into(),
            subject: "Faktura".into(),
            text: "do zapłaty".into(),
            pdfs: Vec::new(),
        }
    }

    async fn job(
        emails: Vec<Email>,
        fail: bool,
    ) -> (EmailWatchJob, Arc<FakeParser>, Arc<payment::Service>) {
        let pool = connect("sqlite::memory:").await.unwrap();
        let payments = Arc::new(payment::Service::new(pool.clone()));
        let parser = Arc::new(FakeParser {
            calls: AtomicUsize::new(0),
            fail: Mutex::new(fail),
        });
        let service = Arc::new(Service::new(parser.clone(), payments.clone(), vec![]));
        let job = EmailWatchJob::new(
            Arc::new(FakeMailbox(emails)),
            service,
            pool,
            EmailWatchConfig::default(),
        );
        (job, parser, payments)
    }

    #[tokio::test]
    async fn each_email_is_processed_once() {
        let (job, parser, payments) = job(vec![email("<a@x>"), email("<b@x>")], false).await;

        job.process().await;
        job.process().await;

        assert_eq!(parser.calls.load(Ordering::SeqCst), 2);
        assert_eq!(payments.list(None).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failing_email_is_retried_up_to_the_limit_then_left_alone() {
        let (job, parser, payments) = job(vec![email("<a@x>")], true).await;

        for _ in 0..5 {
            job.process().await;
        }

        assert_eq!(parser.calls.load(Ordering::SeqCst), 3);
        assert!(payments.list(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_email_that_recovers_before_the_limit_creates_its_payment() {
        let (job, parser, payments) = job(vec![email("<a@x>")], true).await;
        job.process().await;

        *parser.fail.lock().unwrap() = false;
        job.process().await;
        job.process().await;

        assert_eq!(parser.calls.load(Ordering::SeqCst), 2);
        assert_eq!(payments.list(None).await.unwrap().len(), 1);
    }
}
