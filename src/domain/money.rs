use std::str::FromStr;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rusty_money::Money as RustyMoney;
use rusty_money::iso::Currency;

/// Domain type, deliberately serde-free - wire (de)serialization lives in
/// `api::serializers::money::MoneyDto`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Money(RustyMoney<'static, Currency>);

impl Money {
    /// Rejects an amount with more decimal places than the currency has minor units, rather
    /// than rounding it: a silently rounded total is worse than a refused one here.
    pub fn from_parts(currency_code: &str, amount: &str) -> Result<Self, String> {
        let currency = rusty_money::iso::find(currency_code)
            .ok_or_else(|| format!("unknown currency code: {currency_code}"))?;
        let mut amount = Decimal::from_str(amount).map_err(|e| e.to_string())?;
        if amount.normalize().scale() > currency.exponent {
            return Err(format!(
                "{amount} has more decimal places than {currency_code} allows ({})",
                currency.exponent
            ));
        }
        amount.rescale(currency.exponent);
        Ok(Money(RustyMoney::from_decimal(amount, currency)))
    }

    pub fn from_minor(currency_code: &str, minor_amount: u32) -> Result<Self, String> {
        let currency = rusty_money::iso::find(currency_code)
            .ok_or_else(|| format!("unknown currency code: {currency_code}"))?;
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
