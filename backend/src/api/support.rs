use crate::api::models::{GraphStatsResponse, RouteHop, RouteSummary};
use crate::api::state::AppState;
use crate::routing::MAX_SUPPORTED_HOPS;
use crate::types::{DexProtocol, PoolEdge, PoolType};
use std::collections::BTreeMap;

pub fn build_graph_stats_response(state: &AppState) -> GraphStatsResponse {
    let mut dex_pool_counts = BTreeMap::new();
    let mut pool_type_counts = BTreeMap::new();

    for pool in &state.pools {
        *dex_pool_counts.entry(dex_label(pool.dex)).or_default() += 1;
        *pool_type_counts
            .entry(pool_type_label(pool.pool_type))
            .or_default() += 1;
    }

    GraphStatsResponse {
        graph: state.stats.clone(),
        validation: state.validation.clone(),
        dex_pool_counts,
        pool_type_counts,
    }
}

pub fn find_candidate_routes(
    state: &AppState,
    source_mint: &str,
    target_mint: &str,
    max_hops: usize,
    exact_hops: Option<usize>,
    limit: usize,
) -> Option<(Vec<Vec<PoolEdge>>, usize, usize)> {
    if source_mint == target_mint {
        return None;
    }

    let effective_max_hops = max_hops.min(MAX_SUPPORTED_HOPS);
    let limit = limit.clamp(1, 100);
    let mut routes =
        state
            .graph
            .find_routes(source_mint, target_mint, effective_max_hops, Some(limit));

    if let Some(h) = exact_hops {
        routes.retain(|route| route.len() == h);
    }

    Some((routes, effective_max_hops, limit))
}

pub fn summarize_route(source_mint: &str, route: &[PoolEdge]) -> RouteSummary {
    let mut current_mint = source_mint.to_string();
    let mut total_fee_rate = 0.0;
    let mut path = Vec::with_capacity(route.len());

    for pool in route {
        let (from_token, to_token) = if pool.token_a.mint == current_mint {
            (&pool.token_a, &pool.token_b)
        } else {
            (&pool.token_b, &pool.token_a)
        };

        total_fee_rate += pool.fee_rate;
        current_mint = to_token.mint.clone();

        path.push(RouteHop {
            pool_address: pool.address.clone(),
            dex: dex_label(pool.dex),
            pool_type: pool_type_label(pool.pool_type),
            from_mint: from_token.mint.clone(),
            from_symbol: from_token.symbol.clone(),
            to_mint: to_token.mint.clone(),
            to_symbol: to_token.symbol.clone(),
            fee_rate: pool.fee_rate,
            tvl: pool.tvl,
        });
    }

    RouteSummary {
        hops: route.len(),
        total_fee_rate,
        estimated_total_fee_bps: (total_fee_rate * 10_000.0).round() as u64,
        path,
    }
}

fn dex_label(dex: DexProtocol) -> &'static str {
    match dex {
        DexProtocol::Whirlpool => "whirlpool",
        DexProtocol::Raydium => "raydium",
        DexProtocol::Meteora => "meteora",
    }
}

fn pool_type_label(pool_type: PoolType) -> &'static str {
    match pool_type {
        PoolType::Amm => "amm",
        PoolType::ConcentratedLiquidity => "clmm",
        PoolType::Dlmm => "dlmm",
    }
}
