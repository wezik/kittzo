use serde::{Deserialize, Serialize};

use crate::domain::money::Money;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MoneyRepr", into = "MoneyRepr")]
pub struct MoneyDto(pub Money);

#[derive(Serialize, Deserialize)]
struct MoneyRepr {
    minor_amount: u32,
    currency: String,
}

impl TryFrom<MoneyRepr> for MoneyDto {
    type Error = String;

    fn try_from(repr: MoneyRepr) -> Result<Self, Self::Error> {
        Money::from_minor(&repr.currency, repr.minor_amount).map(MoneyDto)
    }
}

impl From<MoneyDto> for MoneyRepr {
    fn from(value: MoneyDto) -> Self {
        MoneyRepr {
            minor_amount: value.0.minor_amount(),
            currency: value.0.currency_code().to_string(),
        }
    }
}

impl From<Money> for MoneyDto {
    fn from(value: Money) -> Self {
        MoneyDto(value)
    }
}

impl From<MoneyDto> for Money {
    fn from(value: MoneyDto) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let original = MoneyDto(Money::from_parts("USD", "10.99").unwrap());
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"{"minor_amount":1099,"currency":"USD"}"#);
        assert_eq!(serde_json::from_str::<MoneyDto>(&json).unwrap(), original);
    }
}
