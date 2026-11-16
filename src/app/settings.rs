use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;

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

    #[error("unexpected persistence error")]
    Persistence,

    #[error(transparent)]
    Secrets(#[from] SecretsError),
}

impl From<persistence::Error> for SettingsError {
    fn from(_: persistence::Error) -> Self {
        SettingsError::Persistence
    }
}

pub struct SettingsService {
    pool: SqlitePool,
    secrets: Arc<dyn Secrets>,
    defaults: HashMap<&'static str, String>,
    overrides: HashMap<&'static str, String>,
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
        }
    }

    pub async fn resolve(&self) -> Result<Settings, SettingsError> {
        let mut stored = repo::all(&self.pool).await?;
        let mut values = HashMap::new();
        for name in NAMES {
            let stored = if SECRETS.contains(&name) {
                self.secrets.get(name).unwrap_or_else(|err| {
                    tracing::error!(%err, name, "failed to read secret");
                    None
                })
            } else {
                stored.remove(name)
            };
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
        for (name, value) in changes {
            let value = value.trim();
            match (SECRETS.contains(&name.as_str()), value.is_empty()) {
                (true, true) => self.secrets.delete(&name)?,
                (true, false) => self.secrets.set(&name, value)?,
                (false, true) => repo::delete(&self.pool, &name).await?,
                (false, false) => repo::set(&self.pool, &name, value).await?,
            }
        }
        Ok(())
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
