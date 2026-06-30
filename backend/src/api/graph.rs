use crate::models::graph::{GraphRoute, GraphRouteStep, RoutesQuery, RoutesResponse};
use crate::models::pool::PubkeyBytes;
use crate::types::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// GET /api/routes?input_mint=...&output_mint=...&max_hops=3&max_routes=50
///
/// Returns all swap paths between two tokens from the cached graph.
/// Routes are sorted by heuristic cost (best first).
pub async fn get_routes(
    State(state): State<AppState>,
    Query(params): Query<RoutesQuery>,
) -> Result<Json<RoutesResponse>, (StatusCode, String)> {
    // Parse input mint
    let input_pubkey = Pubkey::from_str(&params.input_mint)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid input_mint: {e}")))?;
    let input_mint = PubkeyBytes(input_pubkey.to_bytes());

    // Parse output mint
    let output_pubkey = Pubkey::from_str(&params.output_mint)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid output_mint: {e}")))?;
    let output_mint = PubkeyBytes(output_pubkey.to_bytes());

    // Check pools loaded
    if state.pools.read().await.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Pool data not loaded yet. Call GET /api/pools first.".to_string(),
        ));
    }

    // Get or build cached graph
    let builder = state.get_or_build_graph().await;

    let max_hops = params.max_hops.unwrap_or(3);
    let max_routes = params.max_routes.unwrap_or(200);

    // Find all routes (unsorted)
    let mut raw_routes = builder
        .find_all_routes(&input_mint, &output_mint, max_hops, max_routes)
        .unwrap_or_default();

    // Sort by heuristic cost ascending (best first) for the response
    raw_routes.sort_by(|a, b| {
        a.total_cost
            .partial_cmp(&b.total_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Truncate to the requested limit
    raw_routes.truncate(max_routes.max(200));

    // Map the raw graph routes to our simplified API response
    let routes: Vec<GraphRoute> = raw_routes
        .into_iter()
        .filter_map(|route| {
            let steps: Vec<GraphRouteStep> = route
                .steps
                .into_iter()
                .map(|step| {
                    let edge = builder.get_pool_edge(step.pool_id)?;
                    Some(GraphRouteStep {
                        pool_id: step.pool_id,
                        pool_pubkey: edge.pool_pubkey,
                        protocol: edge.protocol,
                        pool_type: edge.pool_type,
                        fee_rate: edge.fee_rate,
                        tvl: edge.tvl,
                        input_mint: step.input_mint,
                        output_mint: step.output_mint,
                    })
                })
                .collect::<Option<Vec<_>>>()?;

            Some(GraphRoute {
                hops: steps.len(),
                estimated_cost: route.total_cost,
                steps,
            })
        })
        .collect();

    Ok(Json(RoutesResponse {
        input_mint: params.input_mint,
        output_mint: params.output_mint,
        routes_found: routes.len(),
        routes,
    }))
}
