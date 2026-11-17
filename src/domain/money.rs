use std::str::FromStr;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rusty_money::Money as RustyMoney;
use rusty_money::iso::Currency;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("unknown currency code: {0}")]
    UnknownCurrency(String),

    #[error("invalid amount: {0}")]
    InvalidAmount(#[from] rust_decimal::Error),

    #[error("{0} is out of range")]
    OutOfRange(Decimal),

    #[error("{amount} has more decimal places than {currency} allows")]
    TooPrecise { amount: Decimal, currency: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Money(RustyMoney<'static, Currency>);

impl Money {
    pub fn from_parts(currency_code: &str, amount: &str) -> Result<Self, Error> {
        let currency = find_currency(currency_code)?;
        let mut amount = Decimal::from_str(amount)?;
        if amount.normalize().scale() > currency.exponent {
            return Err(Error::TooPrecise {
                amount,
                currency: currency_code.to_string(),
            });
        }
        amount.rescale(currency.exponent);
        let minor = amount * Decimal::from(10u64.pow(currency.exponent));
        if minor.is_sign_negative() || minor.to_u32().is_none() {
            return Err(Error::OutOfRange(amount));
        }
        Ok(Money(RustyMoney::from_decimal(amount, currency)))
    }

    pub fn from_minor(currency_code: &str, minor_amount: u32) -> Result<Self, Error> {
        let currency = find_currency(currency_code)?;
        let scale = Decimal::from(10u64.pow(currency.exponent));
        let mut amount = Decimal::from(minor_amount) / scale;
        amount.rescale(currency.exponent);
        Ok(Money(RustyMoney::from_decimal(amount, currency)))
    }

    pub fn amount(&self) -> Decimal {
        *self.0.amount()
    }

    pub fn minor_amount(&self) -> u32 {
        let exponent = self.0.currency().exponent;
        let scale = Decimal::from(10u64.pow(exponent));
        (self.amount() * scale)
            .to_u32()
            .expect("amount overflows u32 minor units")
    }

    pub fn currency_code(&self) -> &'static str {
        self.0.currency().iso_alpha_code
    }
}

fn find_currency(code: &str) -> Result<&'static Currency, Error> {
    rusty_money::iso::find(code).ok_or_else(|| Error::UnknownCurrency(code.to_string()))
}

impl std::fmt::Display for Money {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.amount(), self.currency_code())
    }
}

#[cfg(test)]
pub fn some_money() -> Money {
    Money::from_minor("EUR", 1000).expect("valid money")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(currency: &str, amount: &str) -> Money {
        Money::from_parts(currency, amount).unwrap()
    }

    #[test]
    fn amount_is_rescaled_to_the_currency_exponent() {
        assert_eq!(money("USD", "10.5").to_string(), "10.50 USD");
        assert_eq!(money("USD", "10").to_string(), "10.00 USD");
        assert_eq!(money("JPY", "500").to_string(), "500 JPY");
    }

    #[test]
    fn trailing_zeros_beyond_the_exponent_are_accepted() {
        assert_eq!(money("USD", "10.500"), money("USD", "10.50"));
        assert_eq!(money("JPY", "500.00"), money("JPY", "500"));
    }

    #[test]
    fn too_many_decimal_places_for_the_currency_is_rejected() {
        assert!(Money::from_parts("PLN", "29.4511").is_err());
        assert!(Money::from_parts("USD", "1.005").is_err());
        assert!(Money::from_parts("JPY", "500.25").is_err());
    }

    #[test]
    fn unknown_currency_and_unparsable_amount_are_rejected() {
        assert!(Money::from_parts("XYZ", "1.00").is_err());
        assert!(Money::from_parts("USD", "ten").is_err());
    }

    #[test]
    fn amount_outside_u32_minor_units_is_rejected() {
        assert!(Money::from_parts("PLN", "-5.00").is_err());
        assert!(Money::from_parts("PLN", "50000000.00").is_err());
        assert!(Money::from_parts("PLN", "42949672.95").is_ok());
    }

    #[test]
    fn minor_amount_round_trips_through_from_minor() {
        assert_eq!(money("USD", "10.50").minor_amount(), 1050);
        assert_eq!(money("JPY", "500").minor_amount(), 500);
        assert_eq!(
            Money::from_minor("USD", 1050).unwrap(),
            money("USD", "10.50")
        );
        assert_eq!(Money::from_minor("JPY", 500).unwrap(), money("JPY", "500"));
    }
}
