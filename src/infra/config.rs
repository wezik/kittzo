use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::app::email_watch_job::EmailWatchConfig;
use crate::app::outbox::OutboxRelayConfig;
use crate::app::payment_schedule_job::PaymentScheduleJobConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_port: u16,
    pub log_level: String,
    pub discord: DiscordConfig,
    pub email: EmailConfig,
    pub openrouter: OpenRouterConfig,
    pub payment_schedule: PaymentScheduleJobConfig,
    pub outbox: OutboxRelayConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_port: 7878,
            log_level: "info".to_string(),
            discord: DiscordConfig::default(),
            email: EmailConfig::default(),
            openrouter: OpenRouterConfig::default(),
            payment_schedule: PaymentScheduleJobConfig::default(),
            outbox: OutboxRelayConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscordConfig {
    /// Catch-up only: decisions normally arrive over the gateway within a second.
    pub reconcile_interval_secs: u64,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            reconcile_interval_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmailConfig {
    pub imap_host: String,
    pub label: String,
    #[serde(flatten)]
    pub watch: EmailWatchConfig,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: "imap.gmail.com".to_string(),
            label: "kittzo".to_string(),
            watch: EmailWatchConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub url: String,
    pub model: String,
    pub pdf_engine: String,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            url: "https://openrouter.ai/api/v1/chat/completions".to_string(),
            model: "z-ai/glm-5.3-flash".to_string(),
            pdf_engine: "mistral-ocr".to_string(),
        }
    }
}

const DEFAULT_PATH: &str = "kittzo.toml";

pub fn enter_home_dir() {
    let dir = std::env::var_os("KITTZO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let exe = std::env::current_exe().expect("failed to locate the executable");
            exe.parent()
                .expect("executable has no parent dir")
                .to_path_buf()
        });
    std::env::set_current_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot enter home dir {}: {e}", dir.display()));
    tracing::info!(dir = %dir.display(), "using home dir");
}

impl Config {
    pub fn load() -> Config {
        Config::load_from(Path::new(DEFAULT_PATH))
    }

    fn load_from(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                tracing::info!(path = DEFAULT_PATH, "loading config");
                Config::parse(&contents)
            }
            Err(_) => {
                tracing::warn!(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_email_section() {
        let cfg = Config::parse(
            r#"
            [email]
            imap_host = "imap.example.com"
            "#,
        );
        assert_eq!(cfg.email.imap_host, "imap.example.com");
        assert_eq!(cfg.email.label, "kittzo");
    }

    #[test]
    fn parses_openrouter_section() {
        let cfg = Config::parse(
            r#"
            [openrouter]
            api_key = "sk-test"
            model = "other/model"
            "#,
        );
        assert_eq!(cfg.openrouter.api_key, "sk-test");
        assert_eq!(cfg.openrouter.model, "other/model");
        assert_eq!(
            cfg.openrouter.url,
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(cfg.openrouter.pdf_engine, "mistral-ocr");
    }

    #[test]
    fn committed_kittzo_toml_parses() {
        let cfg = Config::parse(include_str!("../../kittzo.toml"));
        assert_eq!(cfg.server_port, 7878);
        assert_eq!(cfg.openrouter.model, "z-ai/glm-5.3-flash");
        assert_eq!(cfg.email.watch.poll_interval_secs, 30);
        assert_eq!(cfg.email.watch.max_attempts, 3);
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
    fn payment_schedule_section_falls_back_to_defaults_when_absent() {
        let cfg = Config::parse("server_port = 1234\n");
        assert_eq!(cfg.payment_schedule.poll_interval_secs, 3600);
    }

    #[test]
    fn payment_schedule_section_parses_when_present() {
        let cfg = Config::parse("[payment_schedule]\npoll_interval_secs = 60\n");
        assert_eq!(cfg.payment_schedule.poll_interval_secs, 60);
    }

    #[test]
    fn outbox_section_falls_back_to_defaults_when_absent() {
        let cfg = Config::parse("server_port = 1234\n");
        assert_eq!(cfg.outbox.poll_interval_secs, 1);
        assert_eq!(cfg.outbox.batch_limit, 100);
    }

    #[test]
    fn outbox_section_parses_when_present() {
        let cfg = Config::parse(
            "[outbox]\npoll_interval_secs = 2\nretry_after_secs = 10\nbatch_limit = 5\n",
        );
        assert_eq!(cfg.outbox.poll_interval_secs, 2);
        assert_eq!(cfg.outbox.retry_after_secs, 10);
        assert_eq!(cfg.outbox.batch_limit, 5);
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
