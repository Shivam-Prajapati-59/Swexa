use crate::api::models::{
    ErrorResponse, HealthResponse, QuoteQuery, QuoteResponse, QuotedRouteSummary, RouteQuery,
    RoutesResponse,
};
use crate::api::state::AppState;
use crate::api::support::{build_graph_stats_response, find_candidate_routes, summarize_route};
use crate::engine::quote::QuoteEngine;
use crate::routing::MAX_SUPPORTED_HOPS;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;

pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        pool_count: state.pools.len(),
        max_supported_hops: MAX_SUPPORTED_HOPS,
        graph_ready: state.validation.is_valid,
    })
}

pub async fn graph_stats(
    State(state): State<Arc<AppState>>,
) -> Json<crate::api::models::GraphStatsResponse> {
    Json(build_graph_stats_response(&state))
}

pub async fn find_routes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RouteQuery>,
) -> impl IntoResponse {
    let Some((routes, effective_max_hops, limit)) = find_candidate_routes(
        &state,
        &query.source_mint,
        &query.target_mint,
        query.max_hops,
        query.exact_hops,
        query.limit,
    ) else {
        return Err(bad_request("source_mint and target_mint must be different"));
    };

    Ok(Json(RoutesResponse {
        source_mint: query.source_mint.clone(),
        target_mint: query.target_mint.clone(),
        requested_max_hops: query.max_hops,
        effective_max_hops,
        returned_route_count: routes.len().min(limit),
        routes: routes
            .iter()
            .map(|route| summarize_route(&query.source_mint, route))
            .collect(),
    }))
}

pub async fn find_best_quote(
    State(state): State<Arc<AppState>>,
    Query(query): Query<QuoteQuery>,
) -> impl IntoResponse {
    if query.amount_in == 0 {
        return Err(bad_request("amount_in must be greater than zero"));
    }

    let Some((routes, effective_max_hops, _)) = find_candidate_routes(
        &state,
        &query.source_mint,
        &query.target_mint,
        query.max_hops,
        query.exact_hops,
        query.limit,
    ) else {
        return Err(bad_request("source_mint and target_mint must be different"));
    };

    let Some(best) = QuoteEngine::find_best_route(&routes, query.amount_in) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no quoteable route found".to_string(),
            }),
        ));
    };

    let quoted_routes = routes
        .iter()
        .enumerate()
        .filter_map(|(route_index, route)| {
            QuoteEngine::quote_route(route, query.amount_in).map(|quote| QuotedRouteSummary {
                route_index,
                quote,
                route: summarize_route(&query.source_mint, route),
            })
        })
        .collect();

    Ok(Json(QuoteResponse {
        source_mint: query.source_mint.clone(),
        target_mint: query.target_mint.clone(),
        amount_in: query.amount_in,
        requested_max_hops: query.max_hops,
        effective_max_hops,
        candidate_route_count: routes.len(),
        best_route_index: best.best_route_index,
        best_quote: best.quote,
        best_route: summarize_route(&query.source_mint, &routes[best.best_route_index]),
        quoted_routes,
    }))
}

fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
}
