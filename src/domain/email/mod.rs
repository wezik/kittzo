use std::collections::HashSet;

use async_trait::async_trait;

use crate::domain::money::Money;
use crate::domain::payment::{AccountNumber, Details, TaxForm};

#[derive(Clone, Debug, PartialEq)]
pub struct Email {
    pub message_id: String,
    pub from: String,
    pub subject: String,
    pub text: String,
    pub pdfs: Vec<Pdf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pdf {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub title: Option<String>,
    pub amount: String,
    pub currency: String,
    pub invoice_nr: Option<String>,
    pub period: Option<String>,
    pub kind: CandidateKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandidateKind {
    Standard {
        account_number: String,
        recipient: String,
    },
    Tax {
        form: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SenderRule {
    pub from: String,
    pub title: String,
}

#[async_trait]
pub trait EmailParser: Send + Sync {
    async fn parse(&self, email: &Email) -> Result<Vec<Candidate>, ParseError>;
}

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
#[error("failed to parse email: {0}")]
pub struct ParseError(pub String);

#[async_trait]
pub trait Mailbox: Send + Sync {
    async fn fetch_new(&self, known: &HashSet<String>) -> Result<Vec<Email>, MailboxError>;
}

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
#[error("mailbox error: {0}")]
pub struct MailboxError(pub String);

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Skip {
    #[error("invalid amount: {0}")]
    Amount(String),

    #[error("invalid details: {0}")]
    Details(String),

    #[error("unknown tax form {0}")]
    TaxForm(String),

    #[error("title needs {0}, which the email didn't provide")]
    MissingField(&'static str),

    #[error("invalid period {0}, expected YYYY-MM")]
    Period(String),

    #[error("no title")]
    NoTitle,
}

pub fn prepare(
    email: &Email,
    candidate: &Candidate,
    rules: &[SenderRule],
) -> Result<(Money, Details), Skip> {
    let total = Money::from_parts(candidate.currency.trim(), candidate.amount.trim())
        .map_err(Skip::Amount)?;

    let details = match &candidate.kind {
        CandidateKind::Standard {
            account_number,
            recipient,
        } => {
            let title = match rules.iter().find(|rule| matches(rule, &email.from)) {
                Some(rule) => render(
                    &rule.title,
                    candidate.invoice_nr.as_deref(),
                    candidate.period.as_deref(),
                )?,
                None => ai_title(candidate)?,
            };
            if title.trim().is_empty() {
                return Err(Skip::NoTitle);
            }
            let account_number = AccountNumber::from(account_number.clone())
                .map_err(|err| Skip::Details(err.to_string()))?;
            Details::standard(title, account_number, recipient.clone())
                .map_err(|err| Skip::Details(err.to_string()))?
        }
        CandidateKind::Tax { form } => Details::tax(match form.trim() {
            "VAT-7" => TaxForm::Vat7,
            "PPL" => TaxForm::Ppl,
            other => return Err(Skip::TaxForm(other.to_string())),
        }),
    };

    Ok((total, details))
}

fn matches(rule: &SenderRule, from: &str) -> bool {
    let from = from.trim().to_lowercase();
    let pattern = rule.from.trim().to_lowercase();
    if pattern.starts_with('@') {
        from.ends_with(&pattern)
    } else {
        from == pattern
    }
}

fn ai_title(candidate: &Candidate) -> Result<String, Skip> {
    candidate
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .ok_or(Skip::NoTitle)
}

fn render(template: &str, invoice_nr: Option<&str>, period: Option<&str>) -> Result<String, Skip> {
    let invoice_nr = invoice_nr.map(str::trim).filter(|nr| !nr.is_empty());
    let mut title = template.to_string();
    if title.contains("{invoice_nr}") {
        title = title.replace(
            "{invoice_nr}",
            invoice_nr.ok_or(Skip::MissingField("invoice_nr"))?,
        );
    }
    if title.contains("{MM}") || title.contains("{YYYY}") {
        let (year, month) = parse_period(period.ok_or(Skip::MissingField("period"))?)?;
        title = title
            .replace("{MM}", &format!("{month:02}"))
            .replace("{YYYY}", &year.to_string());
    }
    Ok(title)
}

fn parse_period(raw: &str) -> Result<(i32, u8), Skip> {
    let invalid = || Skip::Period(raw.to_string());
    let (year, month) = raw.trim().split_once('-').ok_or_else(invalid)?;
    let year: i32 = year.parse().map_err(|_| invalid())?;
    let month: u8 = month.parse().map_err(|_| invalid())?;
    if !(2000..=2100).contains(&year) || !(1..=12).contains(&month) {
        return Err(invalid());
    }
    Ok((year, month))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NRB: &str = "61109010140000071219812874";

    fn email(from: &str) -> Email {
        Email {
            message_id: "<m1@mail.gmail.com>".into(),
            from: from.into(),
            subject: "Faktura".into(),
            text: String::new(),
            pdfs: Vec::new(),
        }
    }

    fn standard() -> Candidate {
        Candidate {
            title: Some("Faktura za usługi".into()),
            amount: "307.50".into(),
            currency: "PLN".into(),
            invoice_nr: Some("49/09/2026".into()),
            period: Some("2026-09".into()),
            kind: CandidateKind::Standard {
                account_number: NRB.into(),
                recipient: "Biuro Rachunkowe".into(),
            },
        }
    }

    fn tax(form: &str) -> Candidate {
        Candidate {
            kind: CandidateKind::Tax { form: form.into() },
            ..standard()
        }
    }

    fn accountant_rule() -> Vec<SenderRule> {
        vec![SenderRule {
            from: "@biuro.pl".into(),
            title: "Faktura {invoice_nr}".into(),
        }]
    }

    fn title(prepared: Result<(Money, Details), Skip>) -> String {
        match prepared.unwrap() {
            (_, Details::Standard { title, .. }) => title,
            (_, other) => panic!("expected standard details, got {other:?}"),
        }
    }

    #[test]
    fn standard_from_a_ruled_sender_uses_the_template() {
        let prepare = || prepare(&email("Ksiegowa@Biuro.pl"), &standard(), &accountant_rule());
        let (total, _) = prepare().unwrap();
        assert_eq!(total, Money::from_parts("PLN", "307.50").unwrap());
        assert_eq!(title(prepare()), "Faktura 49/09/2026");
    }

    #[test]
    fn exact_address_rule_does_not_match_other_addresses() {
        let rules = vec![SenderRule {
            from: "ksiegowa@biuro.pl".into(),
            title: "Faktura {invoice_nr}".into(),
        }];
        assert_eq!(
            title(prepare(&email("szef@biuro.pl"), &standard(), &rules)),
            "Faktura za usługi"
        );
    }

    #[test]
    fn standard_without_a_rule_keeps_the_ai_title() {
        assert_eq!(
            title(prepare(
                &email("x@other.pl"),
                &standard(),
                &accountant_rule()
            )),
            "Faktura za usługi"
        );
    }

    #[test]
    fn template_placeholder_the_email_lacks_skips_the_payment() {
        let candidate = Candidate {
            invoice_nr: None,
            ..standard()
        };
        assert_eq!(
            prepare(&email("a@biuro.pl"), &candidate, &accountant_rule()),
            Err(Skip::MissingField("invoice_nr"))
        );
    }

    #[test]
    fn period_placeholders_come_from_the_period() {
        let rules = vec![SenderRule {
            from: "@biuro.pl".into(),
            title: "Czynsz {MM}/{YYYY}".into(),
        }];
        assert_eq!(
            title(prepare(&email("a@biuro.pl"), &standard(), &rules)),
            "Czynsz 09/2026"
        );
    }

    #[test]
    fn tax_ignores_sender_rules_and_period() {
        let candidate = Candidate {
            period: None,
            ..tax("VAT-7")
        };
        let (_, details) = prepare(&email("a@biuro.pl"), &candidate, &accountant_rule()).unwrap();
        assert_eq!(details, Details::tax(TaxForm::Vat7));
    }

    #[test]
    fn tax_needs_a_known_form() {
        assert_eq!(
            prepare(&email("a@x.pl"), &tax("PIT-5"), &[]),
            Err(Skip::TaxForm("PIT-5".into()))
        );
    }

    #[test]
    fn invalid_amount_account_or_period_is_skipped() {
        let bad_amount = Candidate {
            amount: "12.345".into(),
            ..standard()
        };
        assert!(matches!(
            prepare(&email("a@x.pl"), &bad_amount, &[]),
            Err(Skip::Amount(_))
        ));
        let bad_account = Candidate {
            kind: CandidateKind::Standard {
                account_number: "123".into(),
                recipient: "X".into(),
            },
            ..standard()
        };
        assert!(matches!(
            prepare(&email("a@x.pl"), &bad_account, &[]),
            Err(Skip::Details(_))
        ));
        let bad_period = Candidate {
            period: Some("09/2026".into()),
            ..standard()
        };
        let rules = vec![SenderRule {
            from: "@x.pl".into(),
            title: "Czynsz {MM}/{YYYY}".into(),
        }];
        assert!(matches!(
            prepare(&email("a@x.pl"), &bad_period, &rules),
            Err(Skip::Period(_))
        ));
    }

    #[test]
    fn malformed_period_is_ignored_when_the_title_does_not_need_it() {
        let candidate = Candidate {
            period: Some("wrzesień".into()),
            ..standard()
        };
        assert!(prepare(&email("a@x.pl"), &candidate, &[]).is_ok());
    }

    #[test]
    fn missing_ai_title_without_a_rule_is_skipped() {
        let candidate = Candidate {
            title: Some("  ".into()),
            ..standard()
        };
        assert_eq!(
            prepare(&email("a@x.pl"), &candidate, &[]),
            Err(Skip::NoTitle)
        );
    }
}
