use axum::{extract::FromRef, routing::get, Router};
use prometheus::Registry;
use rust_template_foundation::server::{health::HealthRegistry, metrics};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
  pub health_registry: HealthRegistry,
  pub metrics_registry: Arc<Registry>,
}

impl FromRef<AppState> for HealthRegistry {
  fn from_ref(state: &AppState) -> Self {
    state.health_registry.clone()
  }
}

impl FromRef<AppState> for Arc<Registry> {
  fn from_ref(state: &AppState) -> Self {
    state.metrics_registry.clone()
  }
}

pub fn base_router(state: AppState) -> Router {
  Router::new()
    .route(
      "/healthz",
      get(rust_template_foundation::server::health::healthz_handler),
    )
    .route("/metrics", get(metrics::metrics_handler))
    .with_state(state)
}
