use crate::graph::builder::GraphBuilder;
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
/// Builds a graph from cached pools and finds all swap paths between two tokens.
/// Returns graph-native routes with pool IDs, fees, TVL, and mint bytes.
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

    // Read pools from shared state
    let pools = state.pools.read().await;
    if pools.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Pool data not loaded yet. Call GET /api/pools first.".to_string(),
        ));
    }

    // Build the graph from cached pools (min TVL $1000 to filter dust)
    let mut builder = GraphBuilder::new();
    builder.build_from_pools(&pools, 1000.0);

    let max_hops = params.max_hops.unwrap_or(3);
    let max_routes = params.max_routes.unwrap_or(200);

    // Find routes (returned sorted by heuristic cost, best first)
    let raw_routes = builder
        .find_all_routes(&input_mint, &output_mint, max_hops, max_routes)
        .unwrap_or_default();

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
