//! CLI configuration — three-stage pipeline:
//!   `CliRaw` (clap) → merge with `ConfigFileRaw` (toml) → `Config` (validated)

use clap::Parser;
use metalps_lib::{LogFormat, LogLevel, SortKey};
use serde::Deserialize;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
  #[error(
    "Failed to read configuration file at {path:?} during startup: {source}"
  )]
  FileRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to parse configuration file at {path:?}: {source}")]
  Parse {
    path: PathBuf,
    #[source]
    source: toml::de::Error,
  },

  #[error("Configuration validation failed: {0}")]
  Validation(String),
}

#[derive(Debug, Parser)]
#[command(
  name = "metalps",
  author,
  version,
  about = "GPU process monitor — like ps, but for the GPU",
  long_about = None,
)]
pub struct CliRaw {
  /// Log level (trace, debug, info, warn, error)
  #[arg(long, env = "LOG_LEVEL")]
  pub log_level: Option<String>,

  /// Log format (text, json)
  #[arg(long, env = "LOG_FORMAT")]
  pub log_format: Option<String>,

  /// Path to configuration file
  #[arg(short, long, env = "CONFIG_FILE")]
  pub config: Option<PathBuf>,

  /// Output as JSON instead of a human-readable table
  #[arg(long, short = 'j')]
  pub json: bool,

  /// Sample interval in milliseconds (how long to measure GPU% over)
  #[arg(long, default_value = "1000", env = "METALPS_INTERVAL")]
  pub interval_ms: Option<u64>,

  /// Only show information for this PID
  #[arg(long, short = 'p')]
  pub pid: Option<i32>,

  /// Sort output by: gpu (default), time, vram, pid
  #[arg(long, default_value = "gpu", env = "METALPS_SORT")]
  pub sort: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ConfigFileRaw {
  pub log_level: Option<String>,
  pub log_format: Option<String>,
  pub interval_ms: Option<u64>,
  pub sort: Option<String>,
}

impl ConfigFileRaw {
  pub fn from_file(path: &PathBuf) -> Result<Self, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|source| {
      ConfigError::FileRead {
        path: path.clone(),
        source,
      }
    })?;

    toml::from_str(&contents).map_err(|source| ConfigError::Parse {
      path: path.clone(),
      source,
    })
  }
}

#[derive(Debug)]
pub struct Config {
  pub log_level: LogLevel,
  pub log_format: LogFormat,
  pub json_output: bool,
  pub interval_ms: u64,
  pub pid_filter: Option<i32>,
  pub sort: SortKey,
}

impl Config {
  pub fn from_cli_and_file(cli: CliRaw) -> Result<Self, ConfigError> {
    let config_file = if let Some(ref path) = cli.config {
      ConfigFileRaw::from_file(path)?
    } else {
      let default = PathBuf::from("config.toml");
      if default.exists() {
        ConfigFileRaw::from_file(&default)?
      } else {
        ConfigFileRaw::default()
      }
    };

    let log_level = cli
      .log_level
      .or(config_file.log_level)
      .unwrap_or_else(|| "warn".to_string())
      .parse::<LogLevel>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    let log_format = cli
      .log_format
      .or(config_file.log_format)
      .unwrap_or_else(|| "text".to_string())
      .parse::<LogFormat>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    let interval_ms = cli
      .interval_ms
      .or(config_file.interval_ms)
      .unwrap_or(1000)
      .max(100); // enforce minimum 100ms to avoid spamming

    let sort = cli
      .sort
      .or(config_file.sort)
      .unwrap_or_else(|| "gpu".to_string())
      .parse::<SortKey>()
      .map_err(ConfigError::Validation)?;

    Ok(Config {
      log_level,
      log_format,
      json_output: cli.json,
      interval_ms,
      pid_filter: cli.pid,
      sort,
    })
  }
}
