use crate::api::handlers::{find_best_quote, find_routes, graph_stats, health};
use crate::api::state::AppState;
use axum::{Router, routing::get};
use std::sync::Arc;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/graph/stats", get(graph_stats))
        .route("/routes", get(find_routes))
        .route("/quote", get(find_best_quote))
        .with_state(state)
}
