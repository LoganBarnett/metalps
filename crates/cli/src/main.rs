//! metalps — GPU process monitor
//!
//! # LLM Development Guidelines
//! When modifying this code:
//! - Keep configuration logic in config.rs
//! - Business logic (collection, formatting) lives in metalps-lib
//! - main.rs only wires things together
//! - Program output goes to stdout; logs go to stderr
//! - Use semantic error types with thiserror - NO anyhow

mod config;
mod logging;

use clap::Parser;
use config::{CliRaw, Config, ConfigError};
use logging::init_logging;
use metalps_lib::{collector, output};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
enum ApplicationError {
  #[error("Failed to load configuration: {0}")]
  Config(#[from] ConfigError),

  #[error("Failed to collect GPU data: {0}")]
  Collect(#[from] collector::CollectorError),

  #[error("Failed to write output: {0}")]
  Output(#[from] std::io::Error),
}

fn main() -> Result<(), ApplicationError> {
  let cli = CliRaw::parse();
  let config = Config::from_cli_and_file(cli).map_err(|e| {
    eprintln!("Configuration error: {e}");
    e
  })?;

  init_logging(config.log_level, config.log_format);
  tracing::debug!("Starting metalps");

  run(config)
}

fn run(config: Config) -> Result<(), ApplicationError> {
  let interval = Duration::from_millis(config.interval_ms);

  let data =
    collector::collect_with_interval(interval, config.sort, config.pid_filter)?;

  let stdout = std::io::stdout();
  let mut out = stdout.lock();

  if config.json_output {
    output::render_json(&mut out, &data)?;
  } else {
    output::render_human(&mut out, &data)?;
  }

  Ok(())
}
