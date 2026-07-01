use crate::graph::optimizer;
use crate::models::graph::{GraphRoute, GraphRouteStep, RankedRoute, RouteQuery, RouteResponse};
use crate::models::pool::PubkeyBytes;
use crate::types::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// GET /api/quote?inputMint=...&outputMint=...&amount=...
/// GET /api/route?input_mint=...&output_mint=...&amount=...
///
/// Returns the top 10 best routes for a given exact input amount,
/// ranked by the estimated output amount from the pool simulator.
pub async fn get_route(
    State(state): State<AppState>,
    Query(params): Query<RouteQuery>,
) -> Result<Json<RouteResponse>, (StatusCode, String)> {
    // Parse input mint
    let input_pubkey = Pubkey::from_str(&params.input_mint)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid input_mint: {e}")))?;
    let input_mint = PubkeyBytes(input_pubkey.to_bytes());

    // Parse output mint
    let output_pubkey = Pubkey::from_str(&params.output_mint)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid output_mint: {e}")))?;
    let output_mint = PubkeyBytes(output_pubkey.to_bytes());

    let amount = params
        .amount
        .parse::<u128>()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid amount: {e}")))?;

    // Reject zero or missing amounts — would produce an empty 200 "no route found"
    if amount == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "amount must be greater than 0".to_string(),
        ));
    }

    // Read pools from shared state — clone and drop the lock immediately
    // so the read guard doesn't block writers during expensive route enumeration.
    let pools = {
        let guard = state.pools.read().await;
        if guard.is_empty() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Pool data not loaded yet. Call GET /api/pools first.".to_string(),
            ));
        }
        guard.clone()
    };

    // Get or build cached graph
    let builder = state.get_or_build_graph().await;

    // Find all raw routes up to the graph-builder hard cap so quote ranking has
    // the broadest candidate set before exact simulation.
    let raw_routes = builder
        .find_all_routes(&input_mint, &output_mint, 4, 20_000)
        .unwrap_or_default();

    // Simulate exact input through each candidate and return the top 10.
    let top_candidates = optimizer::rank_candidates(raw_routes, amount, &pools, 10);

    // Map to API response objects
    let best_routes: Vec<RankedRoute> = top_candidates
        .into_iter()
        .filter_map(|simulated_route| {
            let candidate = simulated_route.candidate;
            let estimated_cost = candidate.total_cost;
            let steps: Option<Vec<GraphRouteStep>> = candidate
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
                .collect();

            let steps = steps?;

            Some(RankedRoute {
                amount_in: amount,
                estimated_amount_out: simulated_route.estimated_amount_out,
                total_fees: simulated_route.total_fees,
                max_price_impact_pct: simulated_route.max_price_impact_pct,
                has_approximate_hops: simulated_route.has_approximate_hops,
                route: GraphRoute {
                    hops: steps.len(),
                    estimated_cost,
                    steps,
                },
                simulated_hops: simulated_route.hops,
            })
        })
        .collect();

    Ok(Json(RouteResponse {
        input_mint: params.input_mint,
        output_mint: params.output_mint,
        amount_in: amount,
        best_routes,
    }))
}
