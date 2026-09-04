use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// Default endpoint used when neither the config file nor `S3_ENDPOINT` are set.
pub const DEFAULT_ENDPOINT: &str = "http://pi:4566";
/// Default region used when neither the config file nor `AWS_REGION` are set.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Application configuration loaded from `$HOME/.config/s3-tui/config.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AwsConfig {
    /// AWS region (overridable via `AWS_REGION`).
    pub region: Option<String>,
    /// S3 endpoint. Most useful for LocalStack / S3-compatible services.
    /// Overridable via `S3_ENDPOINT`.
    pub endpoint: Option<String>,
    /// Named profile from `~/.aws/credentials` to use.
    pub profile: Option<String>,
    /// Use path-style URLs (defaults to `true` whenever an endpoint is set).
    pub force_path_style: Option<bool>,
    /// Static credentials. When omitted, the standard AWS credential chain is
    /// used (env, `~/.aws/credentials`, profiles, etc.).
    pub credentials: Option<StaticCredentials>,
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
    /// Effective region: `AWS_REGION` env var > config file > default.
    pub fn effective_region(&self) -> String {
        std::env::var("AWS_REGION")
            .ok()
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
}

/// Default TOML written on first run: LocalStack-compatible defaults.
pub const DEFAULT_CONFIG_TOML: &str = r#"# s3-tui configuration
region = "us-east-1"
endpoint = "http://pi:4566"
force_path_style = true

[credentials]
access_key_id = "test"
secret_access_key = "test"
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

/// Load the TOML config. A missing file yields the default (empty) config; a
/// malformed file is reported as an error so typos are not silently ignored.
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
        assert_eq!(cfg.endpoint.as_deref(), Some("http://pi:4566"));
        assert_eq!(cfg.force_path_style, Some(true));
        let cred = cfg.credentials.expect("default config has credentials");
        assert_eq!(cred.access_key_id.as_deref(), Some("test"));
        assert_eq!(cred.secret_access_key.as_deref(), Some("test"));
    }

    #[test]
    fn ensure_default_config_creates_file_and_content_parses() {
        let home = std::env::temp_dir().join(format!("s3-tui-test-{}", std::process::id()));
        let path = home.join(".config/s3-tui/config.toml");
        let _ = std::fs::remove_dir_all(&home);

        // Temporarily point $HOME at the temp dir and exercise the function.
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
        assert_eq!(cfg.endpoint.as_deref(), Some("http://pi:4566"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
