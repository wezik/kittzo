use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::watch;

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
    OpenRouterApiKey,
    OpenRouterModel,
}

impl Setting {
    pub const ALL: [Setting; 8] = [
        Setting::DiscordToken,
        Setting::DiscordChannel,
        Setting::ImapHost,
        Setting::ImapUser,
        Setting::ImapAppPassword,
        Setting::ImapLabel,
        Setting::OpenRouterApiKey,
        Setting::OpenRouterModel,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Setting::DiscordToken => "discord_token",
            Setting::DiscordChannel => "discord_channel",
            Setting::ImapHost => "imap_host",
            Setting::ImapUser => "imap_user",
            Setting::ImapAppPassword => "imap_app_password",
            Setting::ImapLabel => "imap_label",
            Setting::OpenRouterApiKey => "openrouter_api_key",
            Setting::OpenRouterModel => "openrouter_model",
        }
    }

    pub fn from_name(name: &str) -> Option<Setting> {
        Setting::ALL.into_iter().find(|key| key.name() == name)
    }

    pub fn is_secret(self) -> bool {
        matches!(
            self,
            Setting::DiscordToken | Setting::ImapAppPassword | Setting::OpenRouterApiKey
        )
    }

    fn env_var(self) -> String {
        format!("KITTZO_{}", self.name().to_uppercase())
    }

    pub fn env_overrides() -> HashMap<Setting, String> {
        Setting::ALL
            .into_iter()
            .filter_map(|key| {
                let value = std::env::var(key.env_var()).ok()?;
                (!value.is_empty()).then_some((key, value))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings(HashMap<Setting, String>);

#[derive(Debug, Clone, PartialEq)]
pub struct DiscordSettings {
    pub token: String,
    pub channel: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmailSettings {
    pub imap_host: String,
    pub imap_user: String,
    pub app_password: String,
    pub label: String,
    pub openrouter_api_key: String,
    pub openrouter_model: String,
}

impl Settings {
    pub fn get(&self, key: Setting) -> &str {
        self.0.get(&key).map_or("", String::as_str)
    }

    fn configured(&self, keys: &[Setting]) -> bool {
        keys.iter().all(|key| self.0.contains_key(key))
    }

    pub fn discord(&self) -> Option<DiscordSettings> {
        self.configured(&[Setting::DiscordToken, Setting::DiscordChannel])
            .then(|| DiscordSettings {
                token: self.get(Setting::DiscordToken).to_string(),
                channel: self.get(Setting::DiscordChannel).to_string(),
            })
    }

    pub fn email(&self) -> Option<EmailSettings> {
        self.configured(&[
            Setting::ImapHost,
            Setting::ImapUser,
            Setting::ImapAppPassword,
            Setting::ImapLabel,
            Setting::OpenRouterApiKey,
            Setting::OpenRouterModel,
        ])
        .then(|| EmailSettings {
            imap_host: self.get(Setting::ImapHost).to_string(),
            imap_user: self.get(Setting::ImapUser).to_string(),
            app_password: self.get(Setting::ImapAppPassword).to_string(),
            label: self.get(Setting::ImapLabel).to_string(),
            openrouter_api_key: self.get(Setting::OpenRouterApiKey).to_string(),
            openrouter_model: self.get(Setting::OpenRouterModel).to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingView {
    pub key: Setting,
    pub value: Option<String>,
    pub is_set: bool,
    pub overridden: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
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
    defaults: HashMap<Setting, String>,
    overrides: HashMap<Setting, String>,
    current: watch::Sender<Settings>,
}

impl SettingsService {
    pub async fn load(
        pool: SqlitePool,
        secrets: Arc<dyn Secrets>,
        defaults: HashMap<Setting, String>,
        overrides: HashMap<Setting, String>,
    ) -> Result<Self, SettingsError> {
        let service = Self {
            pool,
            secrets,
            defaults,
            overrides,
            current: watch::Sender::new(Settings::default()),
        };
        service.reload().await?;
        Ok(service)
    }

    pub fn current(&self) -> Settings {
        self.current.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Settings> {
        self.current.subscribe()
    }

    pub fn view(&self) -> Vec<SettingView> {
        let current = self.current.borrow();
        Setting::ALL
            .into_iter()
            .map(|key| SettingView {
                key,
                value: (!key.is_secret()).then(|| current.get(key).to_string()),
                is_set: !current.get(key).is_empty(),
                overridden: self.overrides.contains_key(&key),
            })
            .collect()
    }

    pub async fn update(&self, changes: HashMap<Setting, String>) -> Result<(), SettingsError> {
        let written = self.write(changes).await;
        self.reload().await?;
        written
    }

    async fn write(&self, changes: HashMap<Setting, String>) -> Result<(), SettingsError> {
        for (key, value) in changes {
            let value = value.trim();
            match (key.is_secret(), value.is_empty()) {
                (true, true) => self.secrets.delete(key.name())?,
                (true, false) => self.secrets.set(key.name(), value)?,
                (false, true) => repo::delete(&self.pool, key.name()).await?,
                (false, false) => repo::set(&self.pool, key.name(), value).await?,
            }
        }
        Ok(())
    }

    async fn reload(&self) -> Result<(), SettingsError> {
        let mut stored = repo::all(&self.pool).await?;
        let mut values = HashMap::new();
        for key in Setting::ALL {
            let stored = if key.is_secret() {
                self.secrets.get(key.name()).unwrap_or_else(|err| {
                    tracing::error!(%err, key = key.name(), "failed to read secret");
                    None
                })
            } else {
                stored.remove(key.name())
            };
            let value = self
                .overrides
                .get(&key)
                .cloned()
                .or(stored)
                .or_else(|| self.defaults.get(&key).cloned())
                .filter(|value| !value.is_empty());
            if let Some(value) = value {
                values.insert(key, value);
            }
        }
        self.current.send_replace(Settings(values));
        Ok(())
    }
}

#[cfg(test)]
pub async fn some_settings(pool: SqlitePool, values: &[(Setting, &str)]) -> Arc<SettingsService> {
    let service = SettingsService::load(
        pool,
        Arc::new(crate::infra::secrets::MemorySecrets::default()),
        HashMap::new(),
        HashMap::new(),
    )
    .await
    .unwrap();
    service
        .update(
            values
                .iter()
                .map(|(key, value)| (*key, value.to_string()))
                .collect(),
        )
        .await
        .unwrap();
    Arc::new(service)
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
                .map(|(key, value)| (*key, value.to_string()))
                .collect()
        };
        SettingsService::load(pool, secrets, map(defaults), map(overrides))
            .await
            .unwrap()
    }

    fn change(key: Setting, value: &str) -> HashMap<Setting, String> {
        HashMap::from([(key, value.to_string())])
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

        service
            .update(HashMap::from([
                (Setting::ImapHost, "stored".to_string()),
                (Setting::ImapLabel, "stored".to_string()),
            ]))
            .await
            .unwrap();

        assert_eq!(service.current().get(Setting::ImapHost), "env");
        assert_eq!(service.current().get(Setting::ImapLabel), "stored");
    }

    #[tokio::test]
    async fn clearing_a_setting_falls_back_to_its_default() {
        let service = service(Arc::default(), &[(Setting::ImapLabel, "default")], &[]).await;

        service
            .update(change(Setting::ImapLabel, "mine"))
            .await
            .unwrap();
        service
            .update(change(Setting::ImapLabel, ""))
            .await
            .unwrap();

        assert_eq!(service.current().get(Setting::ImapLabel), "default");
    }

    #[tokio::test]
    async fn secrets_go_to_the_secret_store_not_the_database() {
        let secrets = Arc::new(MemorySecrets::default());
        let service = service(secrets.clone(), &[], &[]).await;

        service
            .update(change(Setting::DiscordToken, " abc "))
            .await
            .unwrap();

        assert_eq!(
            secrets.get("discord_token").unwrap(),
            Some("abc".to_string())
        );
        assert!(repo::all(&service.pool).await.unwrap().is_empty());
        assert_eq!(service.current().get(Setting::DiscordToken), "abc");

        service
            .update(change(Setting::DiscordToken, ""))
            .await
            .unwrap();
        assert_eq!(secrets.get("discord_token").unwrap(), None);
    }

    #[tokio::test]
    async fn view_never_exposes_secret_values() {
        let service = service(Arc::default(), &[], &[(Setting::OpenRouterApiKey, "sk")]).await;
        service
            .update(change(Setting::ImapUser, "me"))
            .await
            .unwrap();

        let view = service.view();
        let find = |key| view.iter().find(|v| v.key == key).unwrap().clone();

        assert_eq!(
            find(Setting::OpenRouterApiKey),
            SettingView {
                key: Setting::OpenRouterApiKey,
                value: None,
                is_set: true,
                overridden: true,
            }
        );
        assert_eq!(find(Setting::ImapUser).value, Some("me".to_string()));
    }

    #[tokio::test]
    async fn subscribers_see_updates() {
        let service = service(Arc::default(), &[], &[]).await;
        let mut changes = service.subscribe();

        service
            .update(change(Setting::DiscordChannel, "123"))
            .await
            .unwrap();

        assert!(changes.has_changed().unwrap());
        assert_eq!(
            changes.borrow_and_update().get(Setting::DiscordChannel),
            "123"
        );
    }

    #[test]
    fn discord_is_configured_only_with_token_and_channel() {
        let token_only = Settings(HashMap::from([(Setting::DiscordToken, "t".to_string())]));
        let both = Settings(HashMap::from([
            (Setting::DiscordToken, "t".to_string()),
            (Setting::DiscordChannel, "1".to_string()),
        ]));

        assert_eq!(token_only.discord(), None);
        assert_eq!(
            both.discord(),
            Some(DiscordSettings {
                token: "t".into(),
                channel: "1".into()
            })
        );
    }

    #[test]
    fn env_var_names_match_the_documented_ones() {
        assert_eq!(Setting::DiscordToken.env_var(), "KITTZO_DISCORD_TOKEN");
        assert_eq!(
            Setting::ImapAppPassword.env_var(),
            "KITTZO_IMAP_APP_PASSWORD"
        );
        assert_eq!(
            Setting::OpenRouterApiKey.env_var(),
            "KITTZO_OPENROUTER_API_KEY"
        );
    }
}
