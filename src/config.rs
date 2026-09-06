use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_ENDPOINT: &str = "";
pub const DEFAULT_REGION: &str = "us-east-1";

/// Application configuration loaded from `$HOME/.config/s3-tui/config.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AwsConfig {
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub profile: Option<String>,
    pub force_path_style: Option<bool>,
    pub credentials: Option<StaticCredentials>,
    pub ui: Option<UiConfig>,
}

/// UI preferences.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Theme ID used when not overridden by `S3TUI_THEME` / `NO_COLOR`.
    pub theme: Option<String>,
}

/// Static credentials for the S3 client.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StaticCredentials {
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
}

impl AwsConfig {
    /// Effective region: `AWS_REGION` > `AWS_DEFAULT_REGION` > config > default.
    pub fn effective_region(&self) -> String {
        std::env::var("AWS_REGION")
            .ok()
            .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
            .or_else(|| self.region.clone())
            .unwrap_or_else(|| DEFAULT_REGION.to_string())
    }

    /// Effective endpoint: `S3_ENDPOINT` env var > config file > default.
    pub fn effective_endpoint(&self) -> String {
        std::env::var("S3_ENDPOINT")
            .ok()
            .or_else(|| self.endpoint.clone())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
    }

    /// Theme ID from the config file (`[ui] theme`), if set.
    pub fn effective_theme(&self) -> Option<String> {
        self.ui.as_ref().and_then(|ui| ui.theme.clone())
    }
}

/// Default TOML written on first run: standard AWS defaults.
pub const DEFAULT_CONFIG_TOML: &str = r#"# s3-tui configuration
#
# Every value can also be given from the command line (--region, --endpoint,
# --profile, --access-key, --secret-key) or via the usual AWS/S3 environment
# variables (AWS_REGION, AWS_DEFAULT_REGION, S3_ENDPOINT, AWS_PROFILE,
# AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY). Precedence: CLI > env > config.
region = "us-east-1"

# Uncomment for S3-compatible services (LocalStack, MinIO, etc.).
# endpoint = "http://localhost:4566"
# force_path_style = true

[ui]
# Theme ID (built-ins: catppuccin, dracula, gruvbox, nord, one-dark, solarized,
# tailwind, tokyo-night, rose-pine, terminal). Override at runtime with
# S3TUI_THEME; NO_COLOR disables colours entirely.
theme = "catppuccin"

# Uncomment to use static credentials. When omitted, the standard AWS
# credential chain is used (env vars, ~/.aws/credentials, IAM roles, etc.).
# [credentials]
# access_key_id = ""
# secret_access_key = ""
"#;

/// Path of the application config file, honouring `$HOME`.
pub fn config_file_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config/s3-tui/config.toml")
}

/// Create the default config file (and its parent directories) at first run.
/// Returns `true` if the file was created, `false` if it already existed.
pub fn ensure_default_config() -> Result<bool> {
    let path = config_file_path();
    if path.exists() {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    }
    std::fs::write(&path, DEFAULT_CONFIG_TOML)
        .with_context(|| format!("failed to write config file {}", path.display()))?;

    Ok(true)
}

/// Load the TOML config. A missing file yields the default config; a malformed
/// file is reported as an error so typos are not silently ignored.
pub fn load() -> Result<AwsConfig> {
    let path = config_file_path();
    if !path.exists() {
        return Ok(AwsConfig::default());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let config: AwsConfig = toml::from_str(&content)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let raw = r#"
region = "us-east-1"
endpoint = "http://pi:4566"
force_path_style = true

[ui]
theme = "dracula"

[credentials]
access_key_id = "test"
secret_access_key = "secret"
session_token = "token"
"#;
        let cfg: AwsConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.region.as_deref(), Some("us-east-1"));
        assert_eq!(cfg.endpoint.as_deref(), Some("http://pi:4566"));
        assert_eq!(cfg.force_path_style, Some(true));
        assert_eq!(cfg.profile, None);
        let ui = cfg.ui.as_ref().unwrap();
        assert_eq!(ui.theme.as_deref(), Some("dracula"));
        assert_eq!(cfg.effective_theme().as_deref(), Some("dracula"));
        let cred = cfg.credentials.unwrap();
        assert_eq!(cred.access_key_id.as_deref(), Some("test"));
        assert_eq!(cred.secret_access_key.as_deref(), Some("secret"));
        assert_eq!(cred.session_token.as_deref(), Some("token"));
    }

    #[test]
    fn parses_minimal_config() {
        let cfg: AwsConfig = toml::from_str("profile = \"dev\"\n").unwrap();
        assert_eq!(cfg.profile.as_deref(), Some("dev"));
        assert!(cfg.credentials.is_none());
    }

    #[test]
    fn rejects_unknown_fields() {
        let raw = "region = \"us-east-1\"\nbogus_key = 1\n";
        assert!(toml::from_str::<AwsConfig>(raw).is_err());
    }

    #[test]
    fn default_config_toml_is_parseable() {
        let cfg: AwsConfig = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(cfg.region.as_deref(), Some("us-east-1"));
        assert_eq!(
            cfg.effective_theme().as_deref(),
            Some("catppuccin"),
            "default config ships with a theme"
        );
        assert!(
            cfg.credentials.is_none(),
            "standard AWS: default config has no static credentials"
        );
    }

    #[test]
    fn ensure_default_config_creates_file_and_content_parses() {
        let home = std::env::temp_dir().join(format!("s3-tui-test-{}", std::process::id()));
        let path = home.join(".config/s3-tui/config.toml");
        let _ = std::fs::remove_dir_all(&home);

        unsafe {
            std::env::set_var("HOME", &home);
        }
        let created = ensure_default_config().unwrap();
        assert!(created, "config should have been created");
        assert!(path.exists(), "config file should exist");

        let again = ensure_default_config().unwrap();
        assert!(!again, "second call must not recreate");

        let cfg = load().unwrap();
        assert_eq!(cfg.region.as_deref(), Some("us-east-1"));
        assert!(
            cfg.endpoint.is_none(),
            "standard AWS: no endpoint in default config"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn effective_region_prefers_env_vars_over_config() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("AWS_REGION", "sa-east-1");
            std::env::set_var("AWS_DEFAULT_REGION", "eu-central-1");
        }
        let cfg: AwsConfig = toml::from_str("region = \"us-west-2\"\n").unwrap();
        assert_eq!(cfg.effective_region(), "sa-east-1", "AWS_REGION wins");

        unsafe {
            std::env::remove_var("AWS_REGION");
        }
        assert_eq!(
            cfg.effective_region(),
            "eu-central-1",
            "AWS_DEFAULT_REGION is the next fallback"
        );

        unsafe {
            std::env::remove_var("AWS_DEFAULT_REGION");
        }
        assert_eq!(cfg.effective_region(), "us-west-2", "config region");

        assert_eq!(
            AwsConfig::default().effective_region(),
            DEFAULT_REGION,
            "default region"
        );
    }
}
