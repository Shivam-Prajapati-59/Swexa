use crate::api::models::{
    ErrorResponse, HealthResponse, QuoteQuery, QuoteResponse, QuotedRouteSummary, RouteQuery,
    RoutesResponse, SimulatedQuoteResponse, SimulatedQuotedRouteSummary,
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

/// `/quote` endpoint — attempts SVM simulation first, falls back to heuristic.
///
/// Query params:
/// - `heuristic_only=true` forces the old heuristic-only path
/// - Default behavior: simulate → fallback to heuristic on error
pub async fn find_best_quote(
    State(state): State<Arc<AppState>>,
    Query(query): Query<QuoteQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
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

    if routes.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no routes found between the given tokens".to_string(),
            }),
        ));
    }

    // ── Simulation path (default) ──────────────────────────────────────
    if !query.heuristic_only {
        // Step 1: Run the heuristic optimizer to select top-K routes
        let optimized =
            crate::routing::optimizer::select_top_routes(&routes, query.amount_in, None);

        eprintln!(
            "[Optimizer] {} candidates → {} after filtering → top {} selected",
            optimized.total_candidates,
            optimized.after_filtering,
            optimized.top_routes.len(),
        );

        let top_routes = crate::routing::optimizer::extract_routes(&optimized);

        if top_routes.is_empty() {
            eprintln!("[Optimizer] all routes filtered out, falling back to heuristic");
            // Fall through to the heuristic path below
        } else {
            // Step 2: Simulate only the top-K routes
            match state.simulated_engine.find_best_route(
                &top_routes,
                &query.source_mint,
                &query.target_mint,
                query.amount_in,
            ) {
                Ok(sim_result) => {
                    // Map simulator indices (which are relative to top_routes)
                    // back to the original route indices for the response.
                    let best_route_index = sim_result.best.route_index;
                    let best_path = if best_route_index < top_routes.len() {
                        summarize_route(&query.source_mint, &top_routes[best_route_index])
                    } else {
                        summarize_route(
                            &query.source_mint,
                            &top_routes[sim_result
                                .all_quotes
                                .first()
                                .map(|q| q.route_index)
                                .unwrap_or(0)],
                        )
                    };

                    let all_quotes = sim_result
                        .all_quotes
                        .iter()
                        .filter_map(|q| {
                            if q.route_index < top_routes.len() {
                                Some(SimulatedQuotedRouteSummary {
                                    route_index: q.route_index,
                                    amount_out: q.amount_out,
                                    simulated: q.simulated,
                                    fallback_reason: q.fallback_reason.clone(),
                                    route: summarize_route(
                                        &query.source_mint,
                                        &top_routes[q.route_index],
                                    ),
                                })
                            } else {
                                None
                            }
                        })
                        .collect();

                    let response = SimulatedQuoteResponse {
                        quote_method: sim_result.quote_method,
                        source_mint: query.source_mint.clone(),
                        target_mint: query.target_mint.clone(),
                        amount_in: query.amount_in,
                        requested_max_hops: query.max_hops,
                        effective_max_hops,
                        candidate_route_count: optimized.total_candidates,
                        best: sim_result.best,
                        best_path,
                        all_quotes,
                        split: sim_result.split,
                    };

                    return serde_json::to_value(response)
                        .map(|v| Json(v))
                        .map_err(|e| {
                            internal_error(&format!("response serialization failed: {e}"))
                        });
                }
                Err(sim_err) => {
                    eprintln!(
                        "[SimQuote] simulation failed, falling back to heuristic: {}",
                        sim_err
                    );
                    // Fall through to heuristic path below
                }
            }
        }
    }

    // ── Heuristic path (fallback or heuristic_only=true) ───────────────
    let Some(best) = QuoteEngine::find_best_route(&routes, query.amount_in) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no quoteable route found".to_string(),
            }),
        ));
    };

    let mut quoted_routes = routes
        .iter()
        .enumerate()
        .filter_map(|(route_index, route)| {
            QuoteEngine::quote_route(route, query.amount_in).map(|quote| QuotedRouteSummary {
                route_index,
                quote,
                route: summarize_route(&query.source_mint, route),
            })
        })
        .collect::<Vec<_>>();

    quoted_routes.sort_by(|left, right| {
        right
            .quote
            .estimated_amount_out
            .cmp(&left.quote.estimated_amount_out)
            .then_with(|| {
                left.quote
                    .price_impact_bps
                    .cmp(&right.quote.price_impact_bps)
            })
            .then_with(|| left.route.hops.cmp(&right.route.hops))
    });

    let quote_method = if query.heuristic_only {
        "heuristic-phase1"
    } else {
        "heuristic-fallback"
    };

    let best_route = routes.get(best.best_route_index).ok_or_else(|| {
        internal_error(&format!(
            "best_route_index {} out of bounds (routes.len()={})",
            best.best_route_index,
            routes.len()
        ))
    })?;

    let response = QuoteResponse {
        quote_method,
        source_mint: query.source_mint.clone(),
        target_mint: query.target_mint.clone(),
        amount_in: query.amount_in,
        requested_max_hops: query.max_hops,
        effective_max_hops,
        candidate_route_count: routes.len(),
        best_path_index: best.best_route_index,
        best_quote: best.quote,
        best_path: summarize_route(&query.source_mint, best_route),
        quoted_routes,
    };

    serde_json::to_value(response)
        .map(|v| Json(v))
        .map_err(|e| internal_error(&format!("response serialization failed: {e}")))
}

fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
}

fn internal_error(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
}
