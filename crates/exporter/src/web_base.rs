use aide::{
  axum::{routing::get_with, ApiRouter},
  openapi::OpenApi,
  transform::TransformOperation,
};
use axum::{extract::FromRef, Router};
use prometheus::{IntCounter, Registry};
use rust_template_foundation::server::{
  health::HealthRegistry, metrics, openapi,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
  pub health_registry: HealthRegistry,
  pub metrics_registry: Arc<Registry>,
  pub request_counter: IntCounter,
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
  aide::generate::extract_schemas(true);
  let mut api = OpenApi::default();

  let app_router = ApiRouter::new()
    .api_route(
      "/healthz",
      get_with(
        rust_template_foundation::server::health::healthz_handler,
        |op: TransformOperation| op.description("Health check."),
      ),
    )
    .api_route(
      "/metrics",
      get_with(metrics::metrics_handler, |op: TransformOperation| {
        op.description("Prometheus metrics in text/plain format.")
      }),
    )
    .with_state(state)
    .finish_api_with(&mut api, |a| a.title("metalps-exporter"));

  let api = Arc::new(api);

  Router::new()
    .merge(app_router)
    .merge(openapi::openapi_routes(api, "metalps-exporter"))
}
