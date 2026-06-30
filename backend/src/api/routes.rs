use crate::types::AppState;
use axum::{Router, routing::get};

use super::{graph, pool, route};

/// Central API router — add all route groups here.
/// Nested under `/api` in main.rs.
pub fn api_routes() -> Router<AppState> {
    Router::new()
        // Pool routes
        .route("/pools", get(pool::get_all_pools))
        // Graph / routing routes
        .route("/allroutes", get(graph::get_routes))
        // Ranked routes (previously quote)
        .route("/route", get(route::get_route))
    // Future routes go here:
    // .route("/tokens", get(token::list_tokens))
}
