use serde::Deserialize;
use std::path::Path;

use crate::domain::payment_schedule::PaymentScheduleConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_port: u16,
    pub log_level: String,
    payment_schedule: TomlPaymentScheduleConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_port: 7878,
            log_level: "info".to_string(),
            payment_schedule: TomlPaymentScheduleConfig::default(),
        }
    }
}

const DEFAULT_PATH: &str = "kittzo.toml";

impl Config {
    pub fn load() -> Config {
        Config::load_from(Path::new(DEFAULT_PATH))
    }

    pub fn payment_schedule(&self) -> PaymentScheduleConfig {
        self.payment_schedule.clone().into()
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
        toml::from_str(toml_str).expect("invalid config")
    }
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

impl From<TomlPaymentScheduleConfig> for PaymentScheduleConfig {
    fn from(toml: TomlPaymentScheduleConfig) -> Self {
        PaymentScheduleConfig {
            poll_interval_secs: toml.poll_interval_secs,
            retry_after_secs: toml.retry_after_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn payment_schedule_section_falls_back_to_defaults_when_absent() {
        let cfg = Config::parse("server_port = 1234\n");
        assert_eq!(cfg.payment_schedule().poll_interval_secs, 3600);
        assert_eq!(cfg.payment_schedule().retry_after_secs, 1800);
    }

    #[test]
    fn payment_schedule_section_parses_when_present() {
        let cfg =
            Config::parse("[payment_schedule]\npoll_interval_secs = 60\nretry_after_secs = 30\n");
        assert_eq!(cfg.payment_schedule().poll_interval_secs, 60);
        assert_eq!(cfg.payment_schedule().retry_after_secs, 30);
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
