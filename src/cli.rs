use crate::config::{AwsConfig, StaticCredentials};
use clap::Parser;

/// A terminal UI manager for AWS S3.
#[derive(Debug, Clone, Parser)]
#[command(name = "s3-tui", version, about, long_about = None)]
pub struct Cli {
    /// AWS region.
    /// Fallback: `AWS_REGION`/`AWS_DEFAULT_REGION` env vars, then the config
    /// file, then `us-east-1`.
    #[arg(long, value_name = "REGION")]
    pub region: Option<String>,

    /// S3 endpoint URL (e.g. `http://localhost:4566` for LocalStack).
    /// Fallback: `S3_ENDPOINT` env var, then the config file.
    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,

    /// Named profile from `~/.aws/credentials`.
    /// Fallback: `AWS_PROFILE` env var, then the config file.
    #[arg(long, env = "AWS_PROFILE", value_name = "NAME")]
    pub profile: Option<String>,

    /// Static access key (overrides the standard credential chain).
    /// Fallback: `AWS_ACCESS_KEY_ID` env var, then `[credentials]` in config.
    #[arg(long, env = "AWS_ACCESS_KEY_ID", value_name = "ACCESS_KEY")]
    pub access_key: Option<String>,

    /// Static secret key.
    /// Fallback: `AWS_SECRET_ACCESS_KEY` env var, then `[credentials]` in config.
    #[arg(long, env = "AWS_SECRET_ACCESS_KEY", value_name = "SECRET_KEY")]
    pub secret_key: Option<String>,

    /// Session token to use with static credentials.
    /// Fallback: `AWS_SESSION_TOKEN` env var.
    #[arg(long, env = "AWS_SESSION_TOKEN", value_name = "TOKEN")]
    pub session_token: Option<String>,

    /// Force path-style addressing (only meaningful with an endpoint).
    /// Defaults to `force_path_style` from config, or `true` when an endpoint
    /// is in use.
    #[arg(long)]
    pub force_path_style: bool,

    /// Disable path-style addressing (overrides `--force-path-style`).
    #[arg(long)]
    pub no_path_style: bool,
}

/// Fully resolved connection settings after applying the precedence
/// `CLI options > environment > config file > defaults`.
#[derive(Debug, Clone)]
pub struct ResolvedSettings {
    pub region: String,
    pub endpoint: Option<String>,
    pub profile: Option<String>,
    pub force_path_style: bool,
    pub credentials: Option<StaticCredentials>,
}

impl ResolvedSettings {
    /// Resolve every connection setting from CLI flags, environment variables
    /// and the config file (in that order of precedence).
    pub fn from_parts(cli: &Cli, cfg: &AwsConfig) -> Self {
        // `effective_endpoint()` already resolves `S3_ENDPOINT` > config > default.
        let endpoint = cli
            .endpoint
            .clone()
            .or_else(|| {
                let e = cfg.effective_endpoint();
                (!e.is_empty()).then_some(e)
            })
            .filter(|s| !s.is_empty());

        let region = cli.region.clone().unwrap_or_else(|| cfg.effective_region());
        let profile = cli
            .profile
            .clone()
            .or_else(|| cfg.profile.clone())
            .filter(|s| !s.is_empty());

        let force_path_style = if cli.no_path_style {
            false
        } else if cli.force_path_style {
            true
        } else {
            cfg.force_path_style.unwrap_or(true)
        };

        let credentials = match (cli.access_key.as_deref(), cli.secret_key.as_deref()) {
            (Some(ak), Some(sk)) if !ak.is_empty() && !sk.is_empty() => Some(StaticCredentials {
                access_key_id: Some(ak.to_string()),
                secret_access_key: Some(sk.to_string()),
                session_token: cli.session_token.clone().filter(|s| !s.is_empty()),
            }),
            _ => cfg.credentials.clone().and_then(|c| {
                let complete = matches!(
                    (&c.access_key_id, &c.secret_access_key),
                    (Some(ak), Some(sk)) if !ak.is_empty() && !sk.is_empty()
                );
                complete.then_some(c)
            }),
        };

        Self {
            region,
            endpoint,
            profile,
            force_path_style,
            credentials,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn cli_from(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("s3-tui").chain(args.iter().copied())).unwrap()
    }

    fn sample_config() -> AwsConfig {
        let raw = r#"
region = "eu-west-1"
endpoint = "http://cfg:9000"
profile = "dev"
force_path_style = false
[credentials]
access_key_id = "cfg-ak"
secret_access_key = "cfg-sk"
"#;
        toml::from_str(raw).unwrap()
    }

    #[test]
    fn cli_flags_override_config() {
        let cli = cli_from(&[
            "--region",
            "us-west-2",
            "--endpoint",
            "http://cli:9000",
            "--profile",
            "prod",
        ]);
        let s = ResolvedSettings::from_parts(&cli, &sample_config());
        assert_eq!(s.region, "us-west-2");
        assert_eq!(s.endpoint.as_deref(), Some("http://cli:9000"));
        assert_eq!(s.profile.as_deref(), Some("prod"));
    }

    #[test]
    fn config_values_used_when_cli_absent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cli = cli_from(&[]);
        let s = ResolvedSettings::from_parts(&cli, &sample_config());
        assert_eq!(s.region, "eu-west-1");
        assert_eq!(s.endpoint.as_deref(), Some("http://cfg:9000"));
        assert_eq!(s.profile.as_deref(), Some("dev"));
        assert!(
            !s.force_path_style,
            "config force_path_style=false respected"
        );
    }

    #[test]
    fn static_credentials_from_config_are_valid() {
        let cli = cli_from(&[]);
        let s = ResolvedSettings::from_parts(&cli, &sample_config());
        let creds = s.credentials.expect("config credentials used");
        assert_eq!(creds.access_key_id.as_deref(), Some("cfg-ak"));
        assert_eq!(creds.secret_access_key.as_deref(), Some("cfg-sk"));
    }

    #[test]
    fn cli_credentials_override_config() {
        let cli = cli_from(&["--access-key", "cli-ak", "--secret-key", "cli-sk"]);
        let s = ResolvedSettings::from_parts(&cli, &sample_config());
        let creds = s.credentials.expect("cli credentials used");
        assert_eq!(creds.access_key_id.as_deref(), Some("cli-ak"));
        assert_eq!(creds.secret_access_key.as_deref(), Some("cli-sk"));
    }

    #[test]
    fn no_path_style_flag_wins() {
        let cli = cli_from(&["--force-path-style", "--no-path-style"]);
        let s = ResolvedSettings::from_parts(&cli, &sample_config());
        assert!(!s.force_path_style);
    }

    #[test]
    fn endpoint_env_var_falls_back_before_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = AwsConfig::default();
        unsafe {
            env::set_var("S3_ENDPOINT", "http://env:4566");
        }
        let s = ResolvedSettings::from_parts(&cli_from(&[]), &cfg);
        unsafe {
            env::remove_var("S3_ENDPOINT");
        }
        assert_eq!(s.endpoint.as_deref(), Some("http://env:4566"));
    }

    #[test]
    fn profile_env_var_is_picked_up() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::set_var("AWS_PROFILE", "envprofile");
        }
        let s = ResolvedSettings::from_parts(&cli_from(&[]), &AwsConfig::default());
        unsafe {
            env::remove_var("AWS_PROFILE");
        }
        assert_eq!(s.profile.as_deref(), Some("envprofile"));
    }

    #[test]
    fn defaults_apply_with_empty_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let s = ResolvedSettings::from_parts(&cli_from(&[]), &AwsConfig::default());
        assert_eq!(s.region, "us-east-1");
        assert_eq!(s.endpoint.as_deref(), Some("http://pi:4566"));
        assert!(s.force_path_style);
    }

    #[test]
    fn incomplete_cli_credentials_are_ignored() {
        let cli = cli_from(&["--access-key", "only-ak"]);
        let s = ResolvedSettings::from_parts(&cli, &AwsConfig::default());
        assert!(s.credentials.is_none());
    }
}
