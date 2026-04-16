//! metalps-exporter — Prometheus exporter for macOS GPU metrics
//!
//! Periodically samples GPU utilization and memory via metalps-lib and
//! exposes the results at `/metrics` for Prometheus to scrape.

use metalps_exporter::{config, gpu_metrics, web_base};

use axum::{serve, Router};
use clap::Parser;
use config::{CliRaw, Config, ConfigError};
use gpu_metrics::GpuMetrics;
use rust_template_foundation::logging::init_server_logging;
use rust_template_foundation::server::health::HealthRegistry;
use rust_template_foundation::server::{shutdown, systemd};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use web_base::AppState;

#[derive(Debug, Error)]
enum ApplicationError {
  #[error("Failed to load configuration during startup: {0}")]
  ConfigurationLoad(#[from] ConfigError),

  #[error("Failed to bind listener to {address}: {source}")]
  ListenerBind {
    address: String,
    source: std::io::Error,
  },

  #[error("Server encountered a runtime error: {0}")]
  ServerRuntime(#[source] std::io::Error),
}

#[tokio::main]
async fn main() -> Result<(), ApplicationError> {
  let cli = CliRaw::parse();

  let config = Config::from_cli_and_file(cli).map_err(|e| {
    eprintln!("Configuration error: {}", e);
    ApplicationError::ConfigurationLoad(e)
  })?;

  init_server_logging(config.log_level, config.log_format);

  info!("Starting metalps-exporter");
  info!("Binding to {}", config.listen_address);
  info!("GPU sample interval: {}ms", config.interval_ms);

  let gpu_metrics = Arc::new(GpuMetrics::new());
  let _collection_handle = gpu_metrics::spawn_collection_loop(
    gpu_metrics.clone(),
    Duration::from_millis(config.interval_ms),
  );

  let state = AppState {
    health_registry: HealthRegistry::default(),
    metrics_registry: gpu_metrics.registry(),
  };

  let app: Router =
    web_base::base_router(state).layer(TraceLayer::new_for_http());

  let listener = tokio_listener::Listener::bind(
    &config.listen_address,
    &tokio_listener::SystemOptions::default(),
    &tokio_listener::UserOptions::default(),
  )
  .await
  .map_err(|source| {
    error!("Failed to bind to {}: {}", config.listen_address, source);
    ApplicationError::ListenerBind {
      address: config.listen_address.to_string(),
      source,
    }
  })?;

  info!("Server listening on {}", config.listen_address);

  systemd::notify_ready();
  systemd::spawn_watchdog();

  serve(listener, app.into_make_service())
    .with_graceful_shutdown(shutdown::shutdown_signal())
    .await
    .map_err(|e| {
      error!("Server error: {}", e);
      ApplicationError::ServerRuntime(e)
    })?;

  info!("Shutting down metalps-exporter");
  Ok(())
}
