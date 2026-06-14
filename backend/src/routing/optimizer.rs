//! Route Optimizer — heuristic pre-filter and Top-K selection.
//!
//! This module sits between the graph pathfinder (`RouteGraph::find_routes`)
//! and the heavy SVM simulation (`SimulatedQuoteEngine`). Its job is to
//! cheaply score every candidate route using universal pool metadata (TVL and
//! fee_rate) and surface only the best `K` routes for exact simulation.
//!
//! ## Pipeline
//!
//! ```text
//!  Graph Pathfinder          Optimizer               Simulator
//!  ────────────────  ──►  ────────────────  ──►  ────────────────
//!  ~30 raw routes         Top 5 scored            Exact amount_out
//! ```
//!
//! ## Scoring Model
//!
//! For each hop in a route we apply two deductions to the running value:
//!
//! 1. **Base fee**: `value *= (1.0 - fee_rate)`
//! 2. **Slippage estimate**: We model each pool as a simple constant-product
//!    AMM where half the TVL backs the output token. The price impact is:
//!    `slippage = value / (pool_tvl * 0.5 + value)` and we deduct that from
//!    the running value.
//!
//! The final running value is the route's **heuristic score** — a proxy for
//! estimated output. Routes are ranked by this score descending.

use crate::routing::Route;
use crate::types::PoolEdge;

// ── Configuration ─────────────────────────────────────────────────────────

/// Minimum TVL (in USD) for any single pool in a route.
/// Routes containing a pool below this threshold are discarded entirely.
const MIN_POOL_TVL: f64 = 1_000.0;

/// Maximum fee rate for any single hop.
/// A hop with a fee above 1% (0.01) is considered toxic and the route is
/// discarded.
const MAX_HOP_FEE_RATE: f64 = 0.01;

/// Default number of top routes to keep after scoring.
pub const DEFAULT_TOP_K: usize = 5;

// ── Public Types ──────────────────────────────────────────────────────────

/// A scored route ready for ranking.
#[derive(Debug, Clone)]
pub struct ScoredRoute {
    /// Index into the original `routes` slice that was passed to the optimizer.
    pub original_index: usize,
    /// The route itself (sequence of pool edges).
    pub route: Route,
    /// Heuristic estimated output value (higher is better).
    pub heuristic_score: f64,
    /// Number of hops in this route.
    pub hops: usize,
    /// The TVL of the weakest (lowest-TVL) pool in the route.
    pub bottleneck_tvl: f64,
    /// Sum of all hop fee rates.
    pub total_fee_rate: f64,
}

/// Result of the optimization step.
#[derive(Debug, Clone)]
pub struct OptimizedRoutes {
    /// The top-K routes after filtering and scoring, sorted best-first.
    pub top_routes: Vec<ScoredRoute>,
    /// How many raw routes were received from the graph pathfinder.
    pub total_candidates: usize,
    /// How many routes survived the hard cutoff filters.
    pub after_filtering: usize,
}

// ── Core Logic ────────────────────────────────────────────────────────────

/// Filters and scores all candidate routes, returning only the top `K`.
///
/// This is the main entry point for the optimization step.
///
/// # Arguments
/// * `routes`    — Raw routes from `RouteGraph::find_routes`.
/// * `amount_in` — The trade size in raw token units (lamports / smallest unit).
/// * `top_k`     — How many top routes to keep (pass `None` for the default of 5).
pub fn select_top_routes(
    routes: &[Route],
    amount_in: u64,
    top_k: Option<usize>,
) -> OptimizedRoutes {
    let k = top_k.unwrap_or(DEFAULT_TOP_K);
    let total_candidates = routes.len();

    // Step 1: Filter + Score in a single pass
    let mut scored: Vec<ScoredRoute> = routes
        .iter()
        .enumerate()
        .filter_map(|(index, route)| score_route(index, route, amount_in))
        .collect();

    let after_filtering = scored.len();

    // Step 2: Sort by heuristic score descending (best first)
    scored.sort_by(|a, b| {
        b.heuristic_score
            .partial_cmp(&a.heuristic_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Step 3: Keep only top K
    scored.truncate(k);

    OptimizedRoutes {
        top_routes: scored,
        total_candidates,
        after_filtering,
    }
}

/// Extracts only the `Route` vectors from an `OptimizedRoutes` result,
/// preserving the ranked order. This is the slice you pass to the simulator.
pub fn extract_routes(optimized: &OptimizedRoutes) -> Vec<Route> {
    optimized
        .top_routes
        .iter()
        .map(|sr| sr.route.clone())
        .collect()
}

// ── Internal Helpers ──────────────────────────────────────────────────────

/// Scores a single route. Returns `None` if the route fails the hard cutoffs.
fn score_route(original_index: usize, route: &[PoolEdge], amount_in: u64) -> Option<ScoredRoute> {
    if route.is_empty() {
        return None;
    }

    let mut amount = amount_in as f64;
    let mut bottleneck_tvl = f64::MAX;
    let mut total_fee_rate = 0.0;

    for pool in route {
        // ── Hard cutoff: invalid or missing data ──────────────────────
        if !pool.fee_rate.is_finite() || !pool.tvl.is_finite() || pool.tvl <= 0.0 || amount <= 0.0 {
            return None;
        }

        // ── Hard cutoff: minimum TVL ─────────────────────────────────
        if pool.tvl < MIN_POOL_TVL {
            return None;
        }

        // ── Hard cutoff: maximum fee per hop ─────────────────────────
        let fee = pool.fee_rate.clamp(0.0, 1.0);
        if fee > MAX_HOP_FEE_RATE {
            return None;
        }

        // Track aggregate metrics
        total_fee_rate += fee;
        if pool.tvl < bottleneck_tvl {
            bottleneck_tvl = pool.tvl;
        }

        // ── Heuristic scoring ────────────────────────────────────────
        // 1. Deduct the base swap fee
        let amount_after_fee = amount * (1.0 - fee);

        // 2. Estimate slippage using constant-product model.
        //    In a balanced pool, ~50% of TVL is in the output token.
        let pool_output_liquidity = pool.tvl * 0.5;
        let slippage = amount_after_fee / (pool_output_liquidity + amount_after_fee);

        // 3. Apply slippage
        amount = amount_after_fee * (1.0 - slippage);
    }

    Some(ScoredRoute {
        original_index,
        route: route.to_vec(),
        heuristic_score: amount,
        hops: route.len(),
        bottleneck_tvl,
        total_fee_rate,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DexProtocol, PoolType, TokenMint};

    fn token(mint: &str) -> TokenMint {
        TokenMint {
            mint: mint.to_string(),
            symbol: mint.to_string(),
            decimals: 9,
        }
    }

    fn pool(address: &str, tvl: f64, fee_rate: f64) -> PoolEdge {
        PoolEdge {
            address: address.to_string(),
            dex: DexProtocol::Raydium,
            token_a: token("A"),
            token_b: token("B"),
            fee_rate,
            tvl,
            pool_type: PoolType::Amm,
        }
    }

    #[test]
    fn filters_out_low_tvl_routes() {
        let routes = vec![
            vec![pool("good", 50_000.0, 0.003)],
            vec![pool("bad-tvl", 500.0, 0.003)], // below MIN_POOL_TVL
        ];

        let result = select_top_routes(&routes, 1_000, None);
        assert_eq!(result.after_filtering, 1);
        assert_eq!(result.top_routes.len(), 1);
        assert_eq!(result.top_routes[0].original_index, 0);
    }

    #[test]
    fn filters_out_high_fee_routes() {
        let routes = vec![
            vec![pool("good", 50_000.0, 0.003)],
            vec![pool("toxic-fee", 50_000.0, 0.05)], // 5% fee > MAX_HOP_FEE_RATE
        ];

        let result = select_top_routes(&routes, 1_000, None);
        assert_eq!(result.after_filtering, 1);
        assert_eq!(result.top_routes[0].original_index, 0);
    }

    #[test]
    fn prefers_high_tvl_low_fee_route() {
        let routes = vec![
            vec![pool("low-liq", 5_000.0, 0.003)],
            vec![pool("high-liq", 1_000_000.0, 0.003)],
        ];

        let result = select_top_routes(&routes, 100_000, None);
        // The high-liquidity route should score higher
        assert_eq!(result.top_routes[0].original_index, 1);
    }

    #[test]
    fn respects_top_k_limit() {
        let routes: Vec<Route> = (0..20)
            .map(|i| vec![pool(&format!("pool-{i}"), 50_000.0 + i as f64, 0.003)])
            .collect();

        let result = select_top_routes(&routes, 1_000, Some(3));
        assert_eq!(result.top_routes.len(), 3);
        assert_eq!(result.total_candidates, 20);
    }

    #[test]
    fn multi_hop_route_uses_bottleneck_tvl() {
        let routes = vec![vec![
            pool("hop1", 1_000_000.0, 0.001),
            pool("hop2", 5_000.0, 0.001), // bottleneck
        ]];

        let result = select_top_routes(&routes, 1_000, None);
        assert_eq!(result.top_routes.len(), 1);
        assert!((result.top_routes[0].bottleneck_tvl - 5_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_routes_returns_empty() {
        let result = select_top_routes(&[], 1_000, None);
        assert!(result.top_routes.is_empty());
        assert_eq!(result.total_candidates, 0);
    }
}
