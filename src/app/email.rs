use std::sync::Arc;

use crate::app::payment;
use crate::domain::email::{self, Email, Parser};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Parse(#[from] email::Error),

    #[error("failed to create payment: {0}")]
    Payment(#[from] payment::Error),
}

pub struct Service {
    parser: Arc<dyn Parser>,
    payments: Arc<payment::Service>,
}

impl Service {
    pub fn new(parser: Arc<dyn Parser>, payments: Arc<payment::Service>) -> Self {
        Self { parser, payments }
    }

    pub async fn process(&self, email: &Email) -> Result<usize, Error> {
        let results = self.parser.parse(email).await?.results;
        let candidates = results.len();
        let mut created = 0;
        for result in results {
            match result {
                Ok((payment, event)) => {
                    self.payments.insert(payment, event).await?;
                    created += 1;
                }
                Err(err) => tracing::warn!(
                    %err,
                    message_id = %email.external_id,
                    subject = %email.subject,
                    "skipped payment candidate from email"
                ),
            }
        }
        tracing::info!(
            message_id = %email.external_id,
            candidates,
            created,
            "processed email"
        );
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::email::ParsingResult;
    use crate::domain::money::some_money;
    use crate::domain::payment::{self as domain, Details, Payment, Source, TaxForm};
    use crate::infra::persistence::connect;
    use async_trait::async_trait;
    use uuid::Uuid;

    struct FakeParser(Result<Vec<Result<Details, domain::Error>>, email::Error>);

    #[async_trait]
    impl Parser for FakeParser {
        async fn parse(&self, email: &Email) -> Result<ParsingResult, email::Error> {
            let source = Source::Email {
                message_id: email.external_id.clone(),
            };
            let results = match &self.0 {
                Ok(results) => results,
                Err(err) => return Err(err.clone()),
            };
            Ok(ParsingResult {
                results: results
                    .iter()
                    .map(|result| match result {
                        Ok(details) => {
                            Ok(Payment::new(some_money(), details.clone(), source.clone()))
                        }
                        Err(_) => Err(domain::Error::InvalidAccountNumber),
                    })
                    .collect(),
            })
        }
    }

    fn email() -> Email {
        Email {
            id: Uuid::new_v4(),
            external_id: "<m1@mail.gmail.com>".into(),
            from: "ksiegowa@biuro.pl".into(),
            subject: "Rozliczenie 09/2026".into(),
            text: "ZUS, VAT-7 i faktura".into(),
            pdfs: Vec::new(),
        }
    }

    async fn service(
        parsed: Result<Vec<Result<Details, domain::Error>>, email::Error>,
    ) -> (Service, Arc<payment::Service>) {
        let payments = Arc::new(payment::Service::new(
            connect("sqlite::memory:").await.unwrap(),
        ));
        let service = Service::new(Arc::new(FakeParser(parsed)), payments.clone());
        (service, payments)
    }

    #[tokio::test]
    async fn creates_one_payment_per_valid_result_and_skips_the_rest() {
        let (service, payments) = service(Ok(vec![
            Ok(domain::some_standard_details()),
            Ok(Details::tax(TaxForm::Vat7)),
            Err(domain::Error::InvalidAccountNumber),
        ]))
        .await;

        let created = service.process(&email()).await.unwrap();

        assert_eq!(created, 2);
        let stored = payments.list(None).await.unwrap();
        assert!(stored.iter().all(|payment| payment.source
            == Source::Email {
                message_id: "<m1@mail.gmail.com>".into()
            }));
        assert_eq!(
            stored.into_iter().map(|p| p.details).collect::<Vec<_>>(),
            vec![domain::some_standard_details(), Details::tax(TaxForm::Vat7)]
        );
    }

    #[tokio::test]
    async fn an_email_with_nothing_to_pay_creates_nothing() {
        let (service, payments) = service(Ok(vec![])).await;

        assert_eq!(service.process(&email()).await.unwrap(), 0);
        assert!(payments.list(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_parser_failure_is_an_error_so_the_email_gets_retried() {
        let (service, _) = service(Err(email::Error::Parse("timeout".into()))).await;

        assert!(matches!(
            service.process(&email()).await,
            Err(Error::Parse(_))
        ));
    }
}
