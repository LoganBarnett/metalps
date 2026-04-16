use clap::Parser;
use rust_template_foundation::config::{
  find_config_file, load_toml, resolve_log_settings, CommonCli,
  CommonConfigFile, ConfigFileError,
};
use rust_template_foundation::logging::{LogFormat, LogLevel};
use serde::Deserialize;
use thiserror::Error;
use tokio_listener::ListenerAddress;

#[derive(Debug, Error)]
pub enum ConfigError {
  #[error("Failed to load configuration file: {0}")]
  File(#[from] ConfigFileError),

  #[error("Configuration validation failed: {0}")]
  Validation(String),

  #[error("Invalid listen address '{address}': {reason}")]
  InvalidListenAddress {
    address: String,
    reason: &'static str,
  },
}

#[derive(Debug, Parser)]
#[command(author, version, about = "Prometheus exporter for macOS GPU metrics")]
pub struct CliRaw {
  #[command(flatten)]
  pub common: CommonCli,

  /// Address to listen on: host:port for TCP, /path/to.sock for Unix socket,
  /// or sd-listen to inherit a socket from systemd.
  #[arg(long, env = "LISTEN")]
  pub listen: Option<String>,

  /// GPU sample interval in milliseconds.
  #[arg(long, env = "METALPS_INTERVAL")]
  pub interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ConfigFileRaw {
  #[serde(flatten)]
  pub common: CommonConfigFile,

  pub listen: Option<String>,
  pub interval_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Config {
  pub log_level: LogLevel,
  pub log_format: LogFormat,
  pub listen_address: ListenerAddress,
  pub interval_ms: u64,
}

impl Config {
  pub fn from_cli_and_file(cli: CliRaw) -> Result<Self, ConfigError> {
    let config_file: ConfigFileRaw =
      match find_config_file("metalps", cli.common.config.as_deref()) {
        Some(path) => load_toml(&path)?,
        None => ConfigFileRaw::default(),
      };

    let (log_level, log_format) = resolve_log_settings(
      cli.common.log_level,
      cli.common.log_format,
      &config_file.common,
    )
    .map_err(ConfigError::Validation)?;

    let listen_str = cli
      .listen
      .or(config_file.listen)
      .unwrap_or_else(|| "127.0.0.1:9101".to_string());

    let listen_address =
      listen_str.parse::<ListenerAddress>().map_err(|reason| {
        ConfigError::InvalidListenAddress {
          address: listen_str.clone(),
          reason,
        }
      })?;

    let interval_ms =
      cli.interval_ms.or(config_file.interval_ms).unwrap_or(1000);

    Ok(Config {
      log_level,
      log_format,
      listen_address,
      interval_ms,
    })
  }
}
