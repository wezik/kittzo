use serde::{Deserialize, Serialize};

use crate::domain::payment::{AccountNumber, Details, TaxForm};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "DetailsRepr", into = "DetailsRepr")]
pub struct DetailsDto(pub Details);

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "details", rename_all = "snake_case")]
enum DetailsRepr {
    Standard {
        title: String,
        account_number: String,
        recipient: String,
    },
    Tax {
        form_symbol: TaxFormRepr,
    },
}

#[derive(Serialize, Deserialize)]
enum TaxFormRepr {
    #[serde(rename = "VAT-7")]
    Vat7,
    #[serde(rename = "PPL")]
    Ppl,
}

impl TryFrom<DetailsRepr> for DetailsDto {
    type Error = String;

    fn try_from(repr: DetailsRepr) -> Result<Self, Self::Error> {
        match repr {
            DetailsRepr::Standard {
                title,
                account_number,
                recipient,
            } => {
                let number = AccountNumber::from(account_number).map_err(|err| err.to_string())?;
                Details::standard(title, number, recipient).map_err(|err| err.to_string())
            }
            DetailsRepr::Tax { form_symbol } => Ok(Details::tax(match form_symbol {
                TaxFormRepr::Vat7 => TaxForm::Vat7,
                TaxFormRepr::Ppl => TaxForm::Ppl,
            })),
        }
        .map(DetailsDto)
    }
}

impl From<DetailsDto> for DetailsRepr {
    fn from(value: DetailsDto) -> Self {
        match value.0 {
            Details::Standard {
                title,
                account_number,
                recipient,
            } => DetailsRepr::Standard {
                title,
                account_number: account_number.as_str().into(),
                recipient,
            },
            Details::Tax { form } => DetailsRepr::Tax {
                form_symbol: match form {
                    TaxForm::Vat7 => TaxFormRepr::Vat7,
                    TaxForm::Ppl => TaxFormRepr::Ppl,
                },
            },
        }
    }
}

impl From<Details> for DetailsDto {
    fn from(value: Details) -> Self {
        DetailsDto(value)
    }
}

impl From<DetailsDto> for Details {
    fn from(value: DetailsDto) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Serialize, Deserialize)]
    struct Body {
        #[serde(flatten)]
        details: DetailsDto,
    }

    #[test]
    fn standard_wire_shape_round_trips() {
        let wire = json!({
            "type": "standard",
            "details": {
                "title": "ZUS",
                "account_number": "61109010140000071219812874",
                "recipient": "ZUS"
            }
        });
        let body: Body = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(body).unwrap(), wire);
    }

    #[test]
    fn tax_wire_shape_round_trips() {
        let wire = json!({"type": "tax", "details": {"form_symbol": "VAT-7"}});
        let body: Body = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(body).unwrap(), wire);
    }

    #[test]
    fn invalid_account_number_is_rejected() {
        let bad_account = json!({
            "type": "standard",
            "details": {"title": "ZUS", "account_number": "123", "recipient": "ZUS"}
        });
        assert!(serde_json::from_value::<Body>(bad_account).is_err());
    }
}
