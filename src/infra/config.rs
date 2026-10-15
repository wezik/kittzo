use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use crate::domain::email::{EmailWatchConfig, SenderRule};
use crate::domain::outbox::OutboxRelayConfig;
use crate::domain::payment_schedule::PaymentScheduleJobConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_port: u16,
    pub log_level: String,
    pub discord: DiscordConfig,
    pub email: EmailConfig,
    payment_schedule: TomlPaymentScheduleConfig,
    outbox: TomlOutboxConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_port: 7878,
            log_level: "info".to_string(),
            discord: DiscordConfig::default(),
            email: EmailConfig::default(),
            payment_schedule: TomlPaymentScheduleConfig::default(),
            outbox: TomlOutboxConfig::default(),
        }
    }
}

/// `channels` maps a logical channel key (e.g. "confirmations", "logs" - step 19)
/// to the actual Discord channel id, so callers never hardcode ids.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscordConfig {
    pub token: String,
    pub channels: HashMap<String, String>,
    /// Catch-up only: decisions normally arrive over the gateway within a second.
    pub reconcile_interval_secs: u64,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            channels: HashMap::new(),
            reconcile_interval_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_user: String,
    pub app_password: String,
    pub label: String,
    pub poll_interval_secs: u64,
    pub openrouter_api_key: String,
    pub model: String,
    senders: Vec<TomlSenderRule>,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: "imap.gmail.com".to_string(),
            imap_user: String::new(),
            app_password: String::new(),
            label: "kittzo/payments".to_string(),
            poll_interval_secs: EmailWatchConfig::default().poll_interval_secs,
            openrouter_api_key: String::new(),
            model: "z-ai/glm-5.3-flash".to_string(),
            senders: Vec::new(),
        }
    }
}

impl EmailConfig {
    pub fn is_configured(&self) -> bool {
        !self.imap_user.is_empty()
            && !self.app_password.is_empty()
            && !self.openrouter_api_key.is_empty()
    }

    pub fn watch(&self) -> EmailWatchConfig {
        EmailWatchConfig {
            poll_interval_secs: self.poll_interval_secs,
            ..EmailWatchConfig::default()
        }
    }

    pub fn sender_rules(&self) -> Vec<SenderRule> {
        self.senders
            .iter()
            .map(|rule| SenderRule {
                from: rule.from.clone(),
                title: rule.title.clone(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct TomlSenderRule {
    from: String,
    title: String,
}

const DEFAULT_PATH: &str = "kittzo.toml";

impl Config {
    pub fn load() -> Config {
        Config::load_from(Path::new(DEFAULT_PATH))
    }

    pub fn payment_schedule(&self) -> PaymentScheduleJobConfig {
        self.payment_schedule.clone().into()
    }

    pub fn outbox(&self) -> OutboxRelayConfig {
        self.outbox.clone().into()
    }

    fn load_from(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                tracing::info!(path = DEFAULT_PATH, "loading config");
                Config::parse(&contents)
            }
            Err(_) => {
                tracing::info!(
                    path = DEFAULT_PATH,
                    "no config found at the path using defaults"
                );
                Config::default()
            }
        }
    }

    fn parse(toml_str: &str) -> Config {
        toml::from_str(&substitute_env_vars(toml_str)).expect("invalid config")
    }
}

/// Expands `${VAR_NAME}` anywhere in the raw TOML text with that env var's value (empty
/// string if unset) before parsing - lets a value like the Discord token live in
/// `kittzo.toml` as `token = "${KITTZO_DISCORD_TOKEN}"` without hardcoding which fields
/// support it.
fn substitute_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let Some(end) = rest.find('}') else {
            result.push_str("${");
            result.push_str(rest);
            return result;
        };
        if let Ok(value) = std::env::var(&rest[..end]) {
            result.push_str(&value);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

/// Raw TOML shape for `[payment_schedule]` — kept separate from the domain
/// `PaymentScheduleConfig` so the domain type stays serde-free; this is the only
/// place that knows the wire format, converting into the domain type via `From`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
struct TomlPaymentScheduleConfig {
    poll_interval_secs: u64,
    retry_after_secs: u64,
}

impl Default for TomlPaymentScheduleConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 3600,
            retry_after_secs: 1800,
        }
    }
}

impl From<TomlPaymentScheduleConfig> for PaymentScheduleJobConfig {
    fn from(toml: TomlPaymentScheduleConfig) -> Self {
        PaymentScheduleJobConfig {
            poll_interval_secs: toml.poll_interval_secs,
            retry_after_secs: toml.retry_after_secs,
        }
    }
}

/// Raw TOML shape for `[outbox]` - same split as `TomlPaymentScheduleConfig` above.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
struct TomlOutboxConfig {
    poll_interval_secs: u64,
    retry_after_secs: u64,
    batch_limit: i64,
}

impl Default for TomlOutboxConfig {
    fn default() -> Self {
        let defaults = OutboxRelayConfig::default();
        Self {
            poll_interval_secs: defaults.poll_interval_secs,
            retry_after_secs: defaults.retry_after_secs,
            batch_limit: defaults.batch_limit,
        }
    }
}

impl From<TomlOutboxConfig> for OutboxRelayConfig {
    fn from(toml: TomlOutboxConfig) -> Self {
        OutboxRelayConfig {
            poll_interval_secs: toml.poll_interval_secs,
            retry_after_secs: toml.retry_after_secs,
            batch_limit: toml.batch_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_email_section_with_sender_rules() {
        let cfg = Config::parse(
            r#"
            [email]
            openrouter_api_key = "sk-test"

            [[email.senders]]
            from = "@biuro.pl"
            title = "Faktura {invoice_nr}"
            "#,
        );
        assert_eq!(cfg.email.openrouter_api_key, "sk-test");
        assert_eq!(cfg.email.model, "z-ai/glm-5.3-flash");
        assert_eq!(cfg.email.label, "kittzo/payments");
        assert!(!cfg.email.is_configured());
        assert_eq!(
            cfg.email.sender_rules(),
            vec![SenderRule {
                from: "@biuro.pl".into(),
                title: "Faktura {invoice_nr}".into()
            }]
        );
    }

    #[test]
    fn parses_partial_toml_with_defaults() {
        let cfg = Config::parse("server_port = 9000\n");
        assert_eq!(cfg.server_port, 9000);
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn parses_full_toml() {
        let cfg = Config::parse("server_port = 1234\nlog_level = \"debug\"\n");
        assert_eq!(cfg.server_port, 1234);
        assert_eq!(cfg.log_level, "debug");
    }

    #[test]
    fn substitutes_env_var_reference_in_toml() {
        // SAFETY: single-threaded within this test, var name is unique to it.
        unsafe { std::env::set_var("KITTZO_TEST_CONFIG_TOKEN", "secret123") };
        let cfg = Config::parse("[discord]\ntoken = \"${KITTZO_TEST_CONFIG_TOKEN}\"\n");
        unsafe { std::env::remove_var("KITTZO_TEST_CONFIG_TOKEN") };
        assert_eq!(cfg.discord.token, "secret123");
    }

    #[test]
    fn unset_env_var_reference_substitutes_empty_string() {
        let cfg = Config::parse("[discord]\ntoken = \"${KITTZO_TEST_DEFINITELY_UNSET}\"\n");
        assert_eq!(cfg.discord.token, "");
    }

    #[test]
    fn payment_schedule_section_falls_back_to_defaults_when_absent() {
        let cfg = Config::parse("server_port = 1234\n");
        assert_eq!(cfg.payment_schedule().poll_interval_secs, 3600);
        assert_eq!(cfg.payment_schedule().retry_after_secs, 1800);
    }

    #[test]
    fn discord_section_parses_channel_routing_map() {
        let cfg = Config::parse(
            "[discord]\ntoken = \"abc\"\n[discord.channels]\nconfirmations = \"123\"\n",
        );
        assert_eq!(cfg.discord.token, "abc");
        assert_eq!(cfg.discord.channels.get("confirmations").unwrap(), "123");
    }

    #[test]
    fn payment_schedule_section_parses_when_present() {
        let cfg =
            Config::parse("[payment_schedule]\npoll_interval_secs = 60\nretry_after_secs = 30\n");
        assert_eq!(cfg.payment_schedule().poll_interval_secs, 60);
        assert_eq!(cfg.payment_schedule().retry_after_secs, 30);
    }

    #[test]
    fn outbox_section_falls_back_to_defaults_when_absent() {
        let cfg = Config::parse("server_port = 1234\n");
        assert_eq!(cfg.outbox().poll_interval_secs, 1);
        assert_eq!(cfg.outbox().batch_limit, 100);
    }

    #[test]
    fn outbox_section_parses_when_present() {
        let cfg = Config::parse(
            "[outbox]\npoll_interval_secs = 2\nretry_after_secs = 10\nbatch_limit = 5\n",
        );
        assert_eq!(cfg.outbox().poll_interval_secs, 2);
        assert_eq!(cfg.outbox().retry_after_secs, 10);
        assert_eq!(cfg.outbox().batch_limit, 5);
    }

    #[test]
    fn load_falls_back_to_default_when_file_missing() {
        let cfg = Config::load_from(Path::new("definitely-does-not-exist.toml"));
        assert_eq!(cfg.server_port, 7878);
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn load_reads_file_when_present() {
        let path = std::env::temp_dir().join("kittzo_test_config_present.toml");
        std::fs::write(&path, "server_port = 4242\n").unwrap();
        let cfg = Config::load_from(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(cfg.server_port, 4242);
    }

    /// Proves the pattern later steps rely on: a component (e.g. Discord)
    /// owns its own `#[derive(Deserialize, Default)] struct XConfig`, `Config`
    /// gets a `pub x: XConfig` field, and it's populated from a `[x]` table -
    /// no extra loader machinery needed, this is just serde+toml doing its
    /// normal nested-struct deserialization.
    #[test]
    fn nested_table_sections_parse_into_sub_structs() {
        #[derive(Deserialize, Default, PartialEq, Debug)]
        #[serde(default)]
        struct DiscordConfig {
            token: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct WithSection {
            server_port: u16,
            discord: DiscordConfig,
        }

        let parsed: WithSection =
            toml::from_str("server_port = 1\n\n[discord]\ntoken = \"abc\"\n").unwrap();

        assert_eq!(parsed.server_port, 1);
        assert_eq!(parsed.discord.token, "abc");
    }
}
