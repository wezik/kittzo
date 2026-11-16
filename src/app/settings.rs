use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;

use crate::app::transaction::{commit_tx, start_tx};
use crate::infra::persistence::{self, sqlx_settings_repository as repo};
use crate::infra::secrets::{Secrets, SecretsError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Setting {
    DiscordToken,
    DiscordChannel,
    ImapHost,
    ImapUser,
    ImapAppPassword,
    ImapLabel,
    OpenrouterApiKey,
    OpenrouterModel,
}

impl Setting {
    pub const ALL: [Setting; 8] = [
        Setting::DiscordToken,
        Setting::DiscordChannel,
        Setting::ImapHost,
        Setting::ImapUser,
        Setting::ImapAppPassword,
        Setting::ImapLabel,
        Setting::OpenrouterApiKey,
        Setting::OpenrouterModel,
    ];
    pub const DISCORD: [Setting; 2] = [Setting::DiscordToken, Setting::DiscordChannel];
    pub const EMAIL: [Setting; 6] = [
        Setting::ImapHost,
        Setting::ImapUser,
        Setting::ImapAppPassword,
        Setting::ImapLabel,
        Setting::OpenrouterApiKey,
        Setting::OpenrouterModel,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Setting::DiscordToken => "discord_token",
            Setting::DiscordChannel => "discord_channel",
            Setting::ImapHost => "imap_host",
            Setting::ImapUser => "imap_user",
            Setting::ImapAppPassword => "imap_app_password",
            Setting::ImapLabel => "imap_label",
            Setting::OpenrouterApiKey => "openrouter_api_key",
            Setting::OpenrouterModel => "openrouter_model",
        }
    }

    pub fn is_secret(self) -> bool {
        matches!(
            self,
            Setting::DiscordToken | Setting::ImapAppPassword | Setting::OpenrouterApiKey
        )
    }

    fn parse(name: &str) -> Option<Setting> {
        Setting::ALL
            .into_iter()
            .find(|setting| setting.as_str() == name)
    }
}

pub fn env_overrides() -> HashMap<Setting, String> {
    Setting::ALL
        .into_iter()
        .filter_map(|setting| {
            let name = format!("KITTZO_{}", setting.as_str().to_uppercase());
            let value = std::env::var(name).ok()?;
            (!value.is_empty()).then_some((setting, value))
        })
        .collect()
}

pub struct Settings(HashMap<Setting, String>);

impl Settings {
    pub fn get(&self, setting: Setting) -> String {
        self.0.get(&setting).cloned().unwrap_or_default()
    }

    pub fn missing(&self, settings: &[Setting]) -> Vec<&'static str> {
        settings
            .iter()
            .filter(|setting| !self.0.contains_key(setting))
            .map(|setting| setting.as_str())
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
    defaults: HashMap<Setting, String>,
    overrides: HashMap<Setting, String>,
    updating: tokio::sync::Mutex<()>,
}

impl SettingsService {
    pub fn new(
        pool: SqlitePool,
        secrets: Arc<dyn Secrets>,
        defaults: HashMap<Setting, String>,
        overrides: HashMap<Setting, String>,
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
                Setting::ALL
                    .into_iter()
                    .filter(|setting| setting.is_secret())
                    .filter_map(|setting| {
                        let name = setting.as_str();
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
        for setting in Setting::ALL {
            let stored = stored.remove(setting.as_str());
            let value = self
                .overrides
                .get(&setting)
                .cloned()
                .or(stored)
                .or_else(|| self.defaults.get(&setting).cloned())
                .filter(|value| !value.is_empty());
            if let Some(value) = value {
                values.insert(setting, value);
            }
        }
        Ok(Settings(values))
    }

    pub async fn view(&self) -> Result<Vec<SettingView>, SettingsError> {
        let settings = self.resolve().await?;
        Ok(Setting::ALL
            .into_iter()
            .map(|setting| {
                let secret = setting.is_secret();
                let value = settings.get(setting);
                SettingView {
                    name: setting.as_str(),
                    is_set: !value.is_empty(),
                    value: (!secret).then_some(value),
                    secret,
                    overridden: self.overrides.contains_key(&setting),
                }
            })
            .collect())
    }

    pub async fn update(&self, changes: HashMap<String, String>) -> Result<(), SettingsError> {
        let changes = changes
            .into_iter()
            .map(|(name, value)| match Setting::parse(&name) {
                Some(setting) => Ok((setting, value)),
                None => Err(SettingsError::Unknown(name)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (secrets, plain): (Vec<_>, Vec<_>) = changes
            .into_iter()
            .partition(|(setting, _)| setting.is_secret());

        let _serialized = self.updating.lock().await;
        let mut tx = start_tx(&self.pool).await?;
        for (setting, value) in &plain {
            match value.trim() {
                "" => repo::delete(&mut tx, setting.as_str()).await?,
                value => repo::set(&mut tx, setting.as_str(), value).await?,
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

type Previous = Vec<(Setting, Option<String>)>;

fn write_secrets(
    store: &dyn Secrets,
    changes: &[(Setting, String)],
) -> (Previous, Result<(), SecretsError>) {
    let mut previous = Vec::new();
    for (setting, value) in changes {
        let written = store.get(setting.as_str()).and_then(|old| {
            previous.push((*setting, old));
            write_secret(store, setting.as_str(), value.trim())
        });
        if written.is_err() {
            return (previous, written);
        }
    }
    (previous, Ok(()))
}

fn restore_secrets(store: &dyn Secrets, previous: Previous) {
    for (setting, old) in previous.into_iter().rev() {
        let name = setting.as_str();
        if let Err(err) = write_secret(store, name, old.as_deref().unwrap_or("")) {
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
        defaults: &[(Setting, &str)],
        overrides: &[(Setting, &str)],
    ) -> SettingsService {
        let pool = connect("sqlite::memory:").await.unwrap();
        let map = |pairs: &[(Setting, &str)]| {
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
            &[
                (Setting::ImapHost, "default"),
                (Setting::ImapLabel, "default"),
            ],
            &[(Setting::ImapHost, "env")],
        )
        .await;

        service.update(change("imap_host", "stored")).await.unwrap();
        service
            .update(change("imap_label", "stored"))
            .await
            .unwrap();

        let settings = service.resolve().await.unwrap();
        assert_eq!(settings.get(Setting::ImapHost), "env");
        assert_eq!(settings.get(Setting::ImapLabel), "stored");
    }

    #[tokio::test]
    async fn clearing_a_setting_falls_back_to_its_default() {
        let service = service(Arc::default(), &[(Setting::ImapLabel, "default")], &[]).await;

        service.update(change("imap_label", "mine")).await.unwrap();
        service.update(change("imap_label", "")).await.unwrap();

        assert_eq!(
            service.resolve().await.unwrap().get(Setting::ImapLabel),
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
        let service = service(Arc::default(), &[], &[(Setting::OpenrouterApiKey, "sk")]).await;
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
        assert_eq!(settings.missing(&Setting::DISCORD), vec!["discord_channel"]);
    }
}
