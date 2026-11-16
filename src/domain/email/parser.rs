use crate::domain::{
    email::{Email, Error},
    payment::{self, Payment},
};
use async_trait::async_trait;

#[async_trait]
pub trait Parser: Send + Sync {
    async fn parse(&self, email: &Email) -> Result<ParsingResult, Error>;
}

pub type Candidate = Result<(Payment, payment::Event), payment::Error>;

pub struct ParsingResult {
    pub results: Vec<Candidate>,
}
