use crate::domain::payment::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum Details {
    Standard {
        title: String,
        account_number: AccountNumber,
        recipient: String,
    },
    Tax {
        form: TaxForm,
    },
}

impl Details {
    pub fn display_title(&self) -> String {
        match self {
            Details::Standard {
                title, recipient, ..
            } => format!("{title} - {recipient}"),
            Details::Tax {
                form: TaxForm::Vat7,
            } => "VAT-7".into(),
            Details::Tax { form: TaxForm::Ppl } => "PPE".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountNumber(String);

impl AccountNumber {
    pub fn from(raw: String) -> Result<Self, Error> {
        let digits: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        if digits.len() != 26 || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(Error::InvalidAccountNumber);
        }
        let rearranged: u128 = format!("{}2521{}", &digits[2..], &digits[..2])
            .parse()
            .map_err(|_| Error::InvalidAccountNumber)?;
        if rearranged % 97 != 1 {
            return Err(Error::InvalidAccountNumber);
        }

        Ok(AccountNumber(digits))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxForm {
    Vat7,
    Ppl,
}

impl Details {
    pub fn standard(
        title: String,
        account_number: AccountNumber,
        recipient: String,
    ) -> Result<Self, Error> {
        if recipient.trim().is_empty() {
            return Err(Error::InvalidRecipient);
        }

        Ok(Details::Standard {
            title: title,
            account_number: account_number,
            recipient: recipient,
        })
    }

    pub fn tax(form: TaxForm) -> Self {
        Details::Tax { form }
    }
}

#[cfg(test)]
pub fn some_standard_details() -> Details {
    Details::standard(
        "foo".into(),
        AccountNumber::from("61109010140000071219812874".into()).expect("valid account number"),
        "someone".into(),
    )
    .expect("valid details")
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use crate::domain::payment::{AccountNumber, Error};

    #[test]
    fn raw_account_number_is_normalized() {
        // given
        let cases = vec![
            "61 1090 1014 0000 0712 1981 2874",
            "61109010140000 0712 1981 2874",
        ];

        let expected = "61109010140000071219812874";

        // when
        for case in cases {
            let result = AccountNumber::from(case.into()).expect("valid account number");

            // then
            assert_eq!(result.as_str(), expected);
        }
    }

    #[test]
    fn invalid_account_number_is_rejected() {
        // given
        let cases = vec!["61109010140000071212874", "611090101400000712198128741"];

        // when
        for case in cases {
            let result = AccountNumber::from(case.into()).expect_err("invalid account number");

            // then
            assert_matches!(result, Error::InvalidAccountNumber);
        }
    }
}
