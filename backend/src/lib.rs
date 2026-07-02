pub mod adapters;
pub mod api;
pub mod graph;
pub mod hydration;
pub mod models;
pub mod services;
pub mod types;

use axum::Router;
use types::AppState;

/// Builds the HTTP application with shared router state.
///
/// Keeping this in the library target lets integration tests exercise the same
/// route tree as the binary without opening a TCP port.
pub fn build_app(app_state: AppState) -> Router {
    Router::new()
        .nest("/api", api::api_routes())
        .with_state(app_state)
}
