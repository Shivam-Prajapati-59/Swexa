use crate::types::AppState;
use axum::{Router, routing::get};

use super::{graph, pool};

/// Central API router — add all route groups here.
/// Nested under `/api` in main.rs.
pub fn api_routes() -> Router<AppState> {
    Router::new()
        // Pool routes
        .route("/pools", get(pool::get_all_pools))
        // Graph / routing routes
        .route("/routes", get(graph::get_routes))
    // Future routes go here:
    // .route("/quote", post(quote::get_quote))
    // .route("/tokens", get(token::list_tokens))
}
