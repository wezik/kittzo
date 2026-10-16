use std::collections::HashSet;

use async_trait::async_trait;
use uuid::Uuid;

mod parser;
pub use parser::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Email {
    pub id: Uuid,
    pub external_id: String,
    pub from: String,
    pub subject: String,
    pub text: String,
    pub pdfs: Vec<Pdf>,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("failed to parse email: {0}")]
    Parse(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pdf {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[async_trait]
pub trait Mailbox: Send + Sync {
    async fn fetch_new(&self, known: &HashSet<String>) -> Result<Vec<Email>, MailboxError>;
}

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
#[error("mailbox error: {0}")]
pub struct MailboxError(pub String);
