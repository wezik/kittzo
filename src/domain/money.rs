use std::str::FromStr;

use rust_decimal::Decimal;
use rusty_money::Money as RustyMoney;
use rusty_money::iso::Currency;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MoneyRepr", into = "MoneyRepr")]
pub struct Money(RustyMoney<'static, Currency>);

#[derive(Serialize, Deserialize)]
struct MoneyRepr {
    amount: String,
    currency: String,
}

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

    pub fn amount(&self) -> Decimal {
        *self.0.amount()
    }

    pub fn currency_code(&self) -> &'static str {
        self.0.currency().iso_alpha_code
    }
}

impl TryFrom<MoneyRepr> for Money {
    type Error = String;

    fn try_from(repr: MoneyRepr) -> Result<Self, Self::Error> {
        Money::from_parts(&repr.currency, &repr.amount)
    }
}

impl From<Money> for MoneyRepr {
    fn from(money: Money) -> Self {
        MoneyRepr {
            amount: money.amount().to_string(),
            currency: money.currency_code().to_string(),
        }
    }
}

impl std::fmt::Display for Money {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.amount(), self.currency_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(currency: &str, amount: &str) -> Money {
        Money::from_parts(currency, amount).unwrap()
    }

    #[test]
    fn round_trips_through_json() {
        let original = money("USD", "10.99");
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"{"amount":"10.99","currency":"USD"}"#);
        assert_eq!(serde_json::from_str::<Money>(&json).unwrap(), original);
    }

    #[test]
    fn round_trips_a_comma_decimal_locale_currency_written_with_a_dot() {
        let original = money("PLN", "99.99");
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<Money>(&json).unwrap(), original);
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
    fn json_deserialization_rejects_an_over_precise_amount() {
        let json = r#"{"amount":"29.4511","currency":"PLN"}"#;
        assert!(serde_json::from_str::<Money>(json).is_err());
    }
}
