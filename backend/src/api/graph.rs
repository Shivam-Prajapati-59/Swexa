use crate::graph::builder::GraphBuilder;
use crate::models::graph::{
    EnrichedRoute, EnrichedRouteStep, RoutesQuery, RoutesResponse, TokenSummary,
};
use crate::models::pool::{Pool, PubkeyBytes};
use crate::types::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Canonical token info for normalization
// ---------------------------------------------------------------------------

/// Stores the canonical (symbol, name) for a given mint, resolved by picking
/// the label from the pool with the highest TVL — higher TVL pools tend to
/// have more accurate metadata.
struct TokenLabel {
    symbol: String,
    name: String,
}

/// Well-known overrides for mints with inconsistent names across DEXes.
/// wSOL is the most common offender — Whirlpool calls it "Solana",
/// Raydium/Meteora call it "Wrapped SOL".
fn get_known_override(mint: &PubkeyBytes) -> Option<TokenLabel> {
    // So11111111111111111111111111111111111111112
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").ok()?;
    if mint.0 == wsol.to_bytes() {
        return Some(TokenLabel {
            symbol: "SOL".to_string(),
            name: "Wrapped SOL".to_string(),
        });
    }
    None
}

/// Build a canonical mint → (symbol, name) lookup from all pools.
/// For each mint, the label from the highest-TVL pool wins.
fn build_token_registry(pools: &[Pool]) -> HashMap<PubkeyBytes, TokenLabel> {
    // Track (symbol, name, best_tvl) per mint
    let mut registry: HashMap<PubkeyBytes, (String, String, f64)> = HashMap::new();

    for pool in pools {
        let tvl = pool.tvl.unwrap_or(0.0);

        for token in [&pool.metadata.token_a, &pool.metadata.token_b] {
            let entry = registry.entry(token.mint).or_insert_with(|| {
                (token.symbol.clone(), token.name.clone(), tvl)
            });
            // If this pool has higher TVL, prefer its label
            if tvl > entry.2 {
                entry.0 = token.symbol.clone();
                entry.1 = token.name.clone();
                entry.2 = tvl;
            }
        }
    }

    // Apply well-known overrides (wSOL, etc.)
    registry
        .into_iter()
        .map(|(mint, (symbol, name, _tvl))| {
            if let Some(label) = get_known_override(&mint) {
                (mint, label)
            } else {
                (mint, TokenLabel { symbol, name })
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// GET /api/routes?input_mint=...&output_mint=...&max_hops=3&max_routes=50
///
/// Builds a graph from cached pools and finds all swap paths between two tokens.
/// Returns enriched routes with pool addresses, protocol names, and normalized token labels.
pub async fn get_routes(
    State(state): State<AppState>,
    Query(params): Query<RoutesQuery>,
) -> Result<Json<RoutesResponse>, (StatusCode, String)> {
    // Parse input mint
    let input_pubkey = Pubkey::from_str(&params.input_mint).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid input_mint: {e}"),
        )
    })?;
    let input_mint = PubkeyBytes(input_pubkey.to_bytes());

    // Parse output mint
    let output_pubkey = Pubkey::from_str(&params.output_mint).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid output_mint: {e}"),
        )
    })?;
    let output_mint = PubkeyBytes(output_pubkey.to_bytes());

    // Read pools from shared state
    let pools = state.pools.read().await;
    if pools.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Pool data not loaded yet. Call GET /api/pools first.".to_string(),
        ));
    }

    // Build canonical token registry for consistent labels across DEXes
    let token_registry = build_token_registry(&pools);

    // Build a pool_id -> Pool lookup for enrichment
    let pool_by_id: HashMap<u32, &Pool> = pools.iter().map(|p| (p.metadata.id, p)).collect();

    // Build the graph from cached pools (min TVL $100 to filter dust)
    let mut builder = GraphBuilder::new();
    builder.build_from_pools(&pools, 100.0);

    let max_hops = params.max_hops.unwrap_or(3);
    let max_routes = params.max_routes.unwrap_or(50);

    // Find routes
    let raw_routes = builder
        .find_all_routes(&input_mint, &output_mint, max_hops, max_routes)
        .unwrap_or_default();

    // Enrich each route with pool metadata and normalized token labels
    let enriched_routes: Vec<EnrichedRoute> = raw_routes
        .iter()
        .filter_map(|route| {
            let steps: Vec<EnrichedRouteStep> = route
                .steps
                .iter()
                .filter_map(|step| {
                    let pool = pool_by_id.get(&step.pool_id)?;

                    // Look up canonical token labels from the registry
                    let input_label = token_registry.get(&step.input_mint);
                    let output_label = token_registry.get(&step.output_mint);

                    Some(EnrichedRouteStep {
                        pool_address: pool.metadata.pubkey,
                        pool_id: step.pool_id,
                        protocol: format!("{:?}", pool.metadata.protocol),
                        input: TokenSummary {
                            mint: step.input_mint,
                            symbol: input_label
                                .map(|l| l.symbol.clone())
                                .unwrap_or_default(),
                            name: input_label
                                .map(|l| l.name.clone())
                                .unwrap_or_default(),
                        },
                        output: TokenSummary {
                            mint: step.output_mint,
                            symbol: output_label
                                .map(|l| l.symbol.clone())
                                .unwrap_or_default(),
                            name: output_label
                                .map(|l| l.name.clone())
                                .unwrap_or_default(),
                        },
                    })
                })
                .collect();

            // Only include routes where all steps resolved
            if steps.len() == route.steps.len() {
                Some(EnrichedRoute {
                    hops: steps.len(),
                    steps,
                })
            } else {
                None
            }
        })
        .collect();

    Ok(Json(RoutesResponse {
        input_mint: params.input_mint,
        output_mint: params.output_mint,
        routes_found: enriched_routes.len(),
        routes: enriched_routes,
    }))
}
