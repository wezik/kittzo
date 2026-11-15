use keyring::{Entry, Error};

const SERVICE: &str = "kittzo";

#[derive(Debug, thiserror::Error)]
#[error("keyring: {0}")]
pub struct SecretsError(String);

impl From<Error> for SecretsError {
    fn from(err: Error) -> Self {
        SecretsError(err.to_string())
    }
}

pub trait Secrets: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, SecretsError>;
    fn set(&self, name: &str, value: &str) -> Result<(), SecretsError>;
    fn delete(&self, name: &str) -> Result<(), SecretsError>;
}

pub struct Keyring;

impl Secrets for Keyring {
    fn get(&self, name: &str) -> Result<Option<String>, SecretsError> {
        match Entry::new(SERVICE, name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoEntry) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretsError> {
        Ok(Entry::new(SERVICE, name)?.set_password(value)?)
    }

    fn delete(&self, name: &str) -> Result<(), SecretsError> {
        match Entry::new(SERVICE, name)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(test)]
impl Secrets for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<String>, SecretsError> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretsError> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretsError> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}
