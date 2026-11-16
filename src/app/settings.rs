use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;

use crate::app::transaction::{commit_tx, start_tx};
use crate::infra::persistence::{self, sqlx_settings_repository as repo};
use crate::infra::secrets::{Secrets, SecretsError};

const SECRETS: [&str; 3] = ["discord_token", "imap_app_password", "openrouter_api_key"];
const NAMES: [&str; 8] = [
    "discord_token",
    "discord_channel",
    "imap_host",
    "imap_user",
    "imap_app_password",
    "imap_label",
    "openrouter_api_key",
    "openrouter_model",
];

pub const DISCORD: [&str; 2] = ["discord_token", "discord_channel"];
pub const EMAIL: [&str; 6] = [
    "imap_host",
    "imap_user",
    "imap_app_password",
    "imap_label",
    "openrouter_api_key",
    "openrouter_model",
];

pub fn env_overrides() -> HashMap<&'static str, String> {
    NAMES
        .into_iter()
        .filter_map(|name| {
            let value = std::env::var(format!("KITTZO_{}", name.to_uppercase())).ok()?;
            (!value.is_empty()).then_some((name, value))
        })
        .collect()
}

pub struct Settings(HashMap<&'static str, String>);

impl Settings {
    pub fn get(&self, name: &str) -> String {
        self.0.get(name).cloned().unwrap_or_default()
    }

    pub fn missing(&self, names: &[&'static str]) -> Vec<&'static str> {
        names
            .iter()
            .copied()
            .filter(|name| !self.0.contains_key(name))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingView {
    pub name: &'static str,
    pub value: Option<String>,
    pub secret: bool,
    pub is_set: bool,
    pub overridden: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("unknown setting {0}")]
    Unknown(String),

    #[error(transparent)]
    Persistence(#[from] persistence::Error),

    #[error(transparent)]
    Secrets(#[from] SecretsError),
}

pub struct SettingsService {
    pool: SqlitePool,
    secrets: Arc<dyn Secrets>,
    defaults: HashMap<&'static str, String>,
    overrides: HashMap<&'static str, String>,
    updating: tokio::sync::Mutex<()>,
}

impl SettingsService {
    pub fn new(
        pool: SqlitePool,
        secrets: Arc<dyn Secrets>,
        defaults: HashMap<&'static str, String>,
        overrides: HashMap<&'static str, String>,
    ) -> Self {
        Self {
            pool,
            secrets,
            defaults,
            overrides,
            updating: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn resolve(&self) -> Result<Settings, SettingsError> {
        let mut stored = repo::all(&self.pool).await?;
        stored.extend(
            self.blocking(|secrets| {
                SECRETS
                    .into_iter()
                    .filter_map(|name| {
                        let value = secrets.get(name).unwrap_or_else(|err| {
                            tracing::error!(%err, name, "failed to read secret");
                            None
                        })?;
                        Some((name.to_string(), value))
                    })
                    .collect::<Vec<_>>()
            })
            .await,
        );
        let mut values = HashMap::new();
        for name in NAMES {
            let stored = stored.remove(name);
            let value = self
                .overrides
                .get(name)
                .cloned()
                .or(stored)
                .or_else(|| self.defaults.get(name).cloned())
                .filter(|value| !value.is_empty());
            if let Some(value) = value {
                values.insert(name, value);
            }
        }
        Ok(Settings(values))
    }

    pub async fn view(&self) -> Result<Vec<SettingView>, SettingsError> {
        let settings = self.resolve().await?;
        Ok(NAMES
            .into_iter()
            .map(|name| {
                let secret = SECRETS.contains(&name);
                let value = settings.get(name);
                SettingView {
                    name,
                    is_set: !value.is_empty(),
                    value: (!secret).then_some(value),
                    secret,
                    overridden: self.overrides.contains_key(name),
                }
            })
            .collect())
    }

    pub async fn update(&self, changes: HashMap<String, String>) -> Result<(), SettingsError> {
        if let Some(name) = changes.keys().find(|name| !NAMES.contains(&name.as_str())) {
            return Err(SettingsError::Unknown(name.clone()));
        }
        let (secrets, plain): (Vec<_>, Vec<_>) = changes
            .into_iter()
            .partition(|(name, _)| SECRETS.contains(&name.as_str()));

        let _serialized = self.updating.lock().await;
        let mut tx = start_tx(&self.pool).await?;
        for (name, value) in &plain {
            match value.trim() {
                "" => repo::delete(&mut tx, name).await?,
                value => repo::set(&mut tx, name, value).await?,
            }
        }

        let (previous, written) = self
            .blocking(move |store| write_secrets(store, &secrets))
            .await;
        let result = match written {
            Ok(()) => commit_tx(tx).await.map_err(SettingsError::from),
            Err(err) => Err(err.into()),
        };
        if result.is_err() {
            self.blocking(move |store| restore_secrets(store, previous))
                .await;
        }
        result
    }

    async fn blocking<T: Send + 'static>(
        &self,
        op: impl FnOnce(&dyn Secrets) -> T + Send + 'static,
    ) -> T {
        let secrets = self.secrets.clone();
        tokio::task::spawn_blocking(move || op(secrets.as_ref()))
            .await
            .expect("keyring task panicked")
    }
}

type Previous = Vec<(String, Option<String>)>;

fn write_secrets(
    store: &dyn Secrets,
    changes: &[(String, String)],
) -> (Previous, Result<(), SecretsError>) {
    let mut previous = Vec::new();
    for (name, value) in changes {
        let written = store.get(name).and_then(|old| {
            previous.push((name.clone(), old));
            write_secret(store, name, value.trim())
        });
        if written.is_err() {
            return (previous, written);
        }
    }
    (previous, Ok(()))
}

fn restore_secrets(store: &dyn Secrets, previous: Previous) {
    for (name, old) in previous.into_iter().rev() {
        if let Err(err) = write_secret(store, &name, old.as_deref().unwrap_or("")) {
            tracing::error!(%err, name, "failed to restore secret");
        }
    }
}

fn write_secret(store: &dyn Secrets, name: &str, value: &str) -> Result<(), SecretsError> {
    match value {
        "" => store.delete(name),
        value => store.set(name, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::persistence::connect;
    use crate::infra::secrets::MemorySecrets;

    async fn service(
        secrets: Arc<MemorySecrets>,
        defaults: &[(&'static str, &str)],
        overrides: &[(&'static str, &str)],
    ) -> SettingsService {
        let pool = connect("sqlite::memory:").await.unwrap();
        let map = |pairs: &[(&'static str, &str)]| {
            pairs
                .iter()
                .map(|(name, value)| (*name, value.to_string()))
                .collect()
        };
        SettingsService::new(pool, secrets, map(defaults), map(overrides))
    }

    fn change(name: &str, value: &str) -> HashMap<String, String> {
        HashMap::from([(name.to_string(), value.to_string())])
    }

    #[tokio::test]
    async fn override_beats_stored_beats_default() {
        let service = service(
            Arc::default(),
            &[("imap_host", "default"), ("imap_label", "default")],
            &[("imap_host", "env")],
        )
        .await;

        service.update(change("imap_host", "stored")).await.unwrap();
        service
            .update(change("imap_label", "stored"))
            .await
            .unwrap();

        let settings = service.resolve().await.unwrap();
        assert_eq!(settings.get("imap_host"), "env");
        assert_eq!(settings.get("imap_label"), "stored");
    }

    #[tokio::test]
    async fn clearing_a_setting_falls_back_to_its_default() {
        let service = service(Arc::default(), &[("imap_label", "default")], &[]).await;

        service.update(change("imap_label", "mine")).await.unwrap();
        service.update(change("imap_label", "")).await.unwrap();

        assert_eq!(
            service.resolve().await.unwrap().get("imap_label"),
            "default"
        );
    }

    #[tokio::test]
    async fn secrets_go_to_the_secret_store_not_the_database() {
        let secrets = Arc::new(MemorySecrets::default());
        let service = service(secrets.clone(), &[], &[]).await;

        service
            .update(change("discord_token", " abc "))
            .await
            .unwrap();

        assert_eq!(secrets.get("discord_token").unwrap(), Some("abc".into()));
        assert!(repo::all(&service.pool).await.unwrap().is_empty());

        service.update(change("discord_token", "")).await.unwrap();
        assert_eq!(secrets.get("discord_token").unwrap(), None);
    }

    #[tokio::test]
    async fn view_never_exposes_secret_values() {
        let service = service(Arc::default(), &[], &[("openrouter_api_key", "sk")]).await;
        service.update(change("imap_user", "me")).await.unwrap();

        let view = service.view().await.unwrap();
        let find = |name| view.iter().find(|v| v.name == name).unwrap().clone();

        assert_eq!(
            find("openrouter_api_key"),
            SettingView {
                name: "openrouter_api_key",
                value: None,
                secret: true,
                is_set: true,
                overridden: true,
            }
        );
        assert_eq!(find("imap_user").value, Some("me".into()));
    }

    struct FailingSecrets(MemorySecrets, &'static str);

    impl Secrets for FailingSecrets {
        fn get(&self, name: &str) -> Result<Option<String>, SecretsError> {
            self.0.get(name)
        }

        fn set(&self, name: &str, value: &str) -> Result<(), SecretsError> {
            if name == self.1 {
                return Err(SecretsError("locked".into()));
            }
            self.0.set(name, value)
        }

        fn delete(&self, name: &str) -> Result<(), SecretsError> {
            self.0.delete(name)
        }
    }

    #[tokio::test]
    async fn a_failed_update_leaves_nothing_behind() {
        let secrets = Arc::new(FailingSecrets(MemorySecrets::default(), "discord_token"));
        secrets.0.set("openrouter_api_key", "old").unwrap();
        let pool = connect("sqlite::memory:").await.unwrap();
        let service = SettingsService::new(pool, secrets.clone(), HashMap::new(), HashMap::new());

        let result = service
            .update(HashMap::from([
                ("imap_user".to_string(), "me".to_string()),
                ("openrouter_api_key".to_string(), "new".to_string()),
                ("discord_token".to_string(), "t".to_string()),
            ]))
            .await;

        assert!(matches!(result, Err(SettingsError::Secrets(_))));
        assert!(repo::all(&service.pool).await.unwrap().is_empty());
        assert_eq!(
            secrets.get("openrouter_api_key").unwrap(),
            Some("old".into())
        );
    }

    #[tokio::test]
    async fn unknown_settings_are_rejected() {
        let service = service(Arc::default(), &[], &[]).await;

        assert!(matches!(
            service.update(change("bogus", "1")).await,
            Err(SettingsError::Unknown(_))
        ));
    }

    #[tokio::test]
    async fn missing_lists_unset_settings() {
        let service = service(Arc::default(), &[], &[]).await;
        service.update(change("discord_token", "t")).await.unwrap();

        let settings = service.resolve().await.unwrap();
        assert_eq!(settings.missing(&DISCORD), vec!["discord_channel"]);
    }
}
