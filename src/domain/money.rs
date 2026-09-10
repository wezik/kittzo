use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rusty_money::Money as RustyMoney;
use rusty_money::iso::Currency;
use serde::{Deserialize, Serialize};

/// Newtype over [`rusty_money::Money`] so it can carry our own `Serialize`/`Deserialize`
/// and `sqlx::Type` without fighting rusty_money's own representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy)]
#[serde(try_from = "MoneyRepr", into = "MoneyRepr")]
pub struct Money(RustyMoney<'static, Currency>);

#[derive(Serialize, Deserialize)]
struct MoneyRepr {
    amount: String,
    currency: String,
}

impl Money {
    pub fn from_minor(minor_units: i64, currency: &'static Currency) -> Self {
        Self(RustyMoney::from_minor(minor_units, currency))
    }

    pub fn currency(&self) -> &'static Currency {
        self.0.currency()
    }

    pub fn minor_units(&self) -> i64 {
        let scale = Decimal::from(10u64.pow(self.currency().exponent));
        (self.0.amount() * scale).round().to_i64().unwrap_or(0)
    }
}

impl Money {
    fn from_parts(currency_code: &str, amount: &str) -> Result<Self, String> {
        let currency = rusty_money::iso::find(currency_code)
            .ok_or_else(|| format!("unknown currency code: {currency_code}"))?;
        RustyMoney::from_str(amount, currency)
            .map(Money)
            .map_err(|e| e.to_string())
    }
}

impl TryFrom<MoneyRepr> for Money {
    type Error = String;

    fn try_from(repr: MoneyRepr) -> Result<Self, Self::Error> {
        Money::from_parts(&repr.currency, &repr.amount)
    }
}

/// `"<ISO code> <amount>"`, e.g. `"USD 10.99"` — the wire format infra's `sqlx::Type` impl
/// stores as a single TEXT column.
impl std::fmt::Display for Money {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.currency().iso_alpha_code, self.0.amount())
    }
}

impl std::str::FromStr for Money {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (code, amount) = s
            .split_once(' ')
            .ok_or_else(|| format!("invalid money string: {s}"))?;
        Money::from_parts(code, amount)
    }
}

impl From<Money> for MoneyRepr {
    fn from(money: Money) -> Self {
        MoneyRepr {
            amount: money.0.amount().to_string(),
            currency: money.currency().iso_alpha_code.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_money::iso;

    #[test]
    fn round_trips_through_json() {
        let original = Money::from_minor(1099, iso::USD);
        let json = serde_json::to_string(&original).unwrap();
        let restored: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn usd_minor_units_are_cents() {
        let money = Money::from_minor(1099, iso::USD);
        assert_eq!(money.minor_units(), 1099);
        assert_eq!(money.0.amount().to_string(), "10.99");
    }

    #[test]
    fn jpy_minor_units_have_no_decimal_places() {
        let money = Money::from_minor(500, iso::JPY);
        assert_eq!(money.minor_units(), 500);
        assert_eq!(money.0.amount().to_string(), "500");
    }
}
