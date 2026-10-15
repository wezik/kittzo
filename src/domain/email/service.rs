use std::sync::Arc;

use crate::domain::payment::{self, Source};

use super::{Email, EmailParser, ParseError, SenderRule, prepare};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("failed to create payment: {0}")]
    Payment(#[from] payment::Error),
}

pub struct Service {
    parser: Arc<dyn EmailParser>,
    payments: Arc<payment::Service>,
    rules: Vec<SenderRule>,
}

impl Service {
    pub fn new(
        parser: Arc<dyn EmailParser>,
        payments: Arc<payment::Service>,
        rules: Vec<SenderRule>,
    ) -> Self {
        Self {
            parser,
            payments,
            rules,
        }
    }

    pub async fn process(&self, email: &Email) -> Result<usize, Error> {
        let candidates = self.parser.parse(email).await?;
        let mut created = 0;
        for candidate in &candidates {
            match prepare(email, candidate, &self.rules) {
                Ok((total, details)) => {
                    let source = Source::Email {
                        message_id: email.message_id.clone(),
                    };
                    self.payments.create(total, details, source).await?;
                    created += 1;
                }
                Err(skip) => tracing::warn!(
                    %skip,
                    message_id = %email.message_id,
                    subject = %email.subject,
                    ?candidate,
                    "skipped payment candidate from email"
                ),
            }
        }
        tracing::info!(
            message_id = %email.message_id,
            candidates = candidates.len(),
            created,
            "processed email"
        );
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::email::{Candidate, CandidateKind};
    use crate::domain::payment::{Details, Payment, TaxForm};
    use crate::infra::persistence::connect;
    use async_trait::async_trait;

    struct FakeParser(Result<Vec<Candidate>, ParseError>);

    #[async_trait]
    impl EmailParser for FakeParser {
        async fn parse(&self, _email: &Email) -> Result<Vec<Candidate>, ParseError> {
            self.0.clone()
        }
    }

    fn email() -> Email {
        Email {
            message_id: "<m1@mail.gmail.com>".into(),
            from: "ksiegowa@biuro.pl".into(),
            subject: "Rozliczenie 09/2026".into(),
            text: "ZUS, VAT-7 i faktura".into(),
            pdfs: Vec::new(),
        }
    }

    fn candidate(kind: CandidateKind) -> Candidate {
        Candidate {
            title: Some("from ai".into()),
            amount: "100.00".into(),
            currency: "PLN".into(),
            invoice_nr: Some("49/09/2026".into()),
            period: Some("2026-09".into()),
            kind,
        }
    }

    async fn service(
        parsed: Result<Vec<Candidate>, ParseError>,
    ) -> (Service, Arc<payment::Service>) {
        let payments = Arc::new(payment::Service::new(
            connect("sqlite::memory:").await.unwrap(),
        ));
        let rules = vec![SenderRule {
            from: "@biuro.pl".into(),
            title: "Faktura {invoice_nr}".into(),
        }];
        let service = Service::new(Arc::new(FakeParser(parsed)), payments.clone(), rules);
        (service, payments)
    }

    fn details(payments: Vec<Payment>) -> Vec<Details> {
        payments
            .into_iter()
            .map(|payment| {
                assert_eq!(
                    payment.source,
                    Source::Email {
                        message_id: "<m1@mail.gmail.com>".into()
                    }
                );
                payment.details
            })
            .collect()
    }

    #[tokio::test]
    async fn creates_one_payment_per_valid_candidate_and_skips_the_rest() {
        let (service, payments) = service(Ok(vec![
            candidate(CandidateKind::Standard {
                account_number: "61109010140000071219812874".into(),
                recipient: "Biuro".into(),
            }),
            candidate(CandidateKind::Tax {
                form: "VAT-7".into(),
            }),
            candidate(CandidateKind::Standard {
                account_number: "123".into(),
                recipient: "Biuro".into(),
            }),
        ]))
        .await;

        let created = service.process(&email()).await.unwrap();

        assert_eq!(created, 2);
        assert_eq!(
            details(payments.list(None).await.unwrap()),
            vec![
                Details::standard(
                    "Faktura 49/09/2026".into(),
                    "61109010140000071219812874".into(),
                    "Biuro".into()
                )
                .unwrap(),
                Details::tax(TaxForm::Vat7),
            ]
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
        let (service, _) = service(Err(ParseError("timeout".into()))).await;

        assert!(matches!(
            service.process(&email()).await,
            Err(Error::Parse(_))
        ));
    }
}
