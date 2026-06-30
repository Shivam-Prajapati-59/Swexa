use crate::models::pool::{DexProtocol, Pool, PoolId, PoolType, PubkeyBytes};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Query parameters for GET /api/routes
/// Example: /api/routes?input_mint=So11...112&output_mint=EPjFW...1v&max_hops=3&max_routes=50
#[derive(Debug, Deserialize)]
pub struct RoutesQuery {
    /// Base58 encoded input token mint address
    pub input_mint: String,
    /// Base58 encoded output token mint address
    pub output_mint: String,
    /// Maximum number of swaps in a route (default: 3, max: 4)
    pub max_hops: Option<usize>,
    /// Maximum number of routes to return (default: 50)
    pub max_routes: Option<usize>,
}

// ---------------------------------------------------------------------------
// Graph edge weight
// ---------------------------------------------------------------------------

/// Rich edge weight stored directly on the graph.
///
/// Contains everything needed for routing decisions WITHOUT
/// looking up the full `Pool` object. This is the core upgrade
/// from the previous `PoolId`-only edges.
///
/// With `PoolEdge` on the graph, the pathfinding algorithm can:
/// - Sort candidate routes by `heuristic_cost()` (fee + 1/TVL)
/// - Skip low-quality edges during expansion
/// - Provide enriched API responses without a separate pool lookup
#[derive(Debug, Clone, Serialize)]
pub struct PoolEdge {
    /// Internal router pool id
    pub pool_id: PoolId,
    /// Solana pubkey of the pool
    pub pool_pubkey: PubkeyBytes,
    /// Protocol name
    pub protocol: DexProtocol,
    /// Type of the pool
    pub pool_type: PoolType,
    /// Total Value Locked in USD
    pub tvl: f64,
    /// Protocol-native fee representation
    pub fee_rate: u32,
}

impl PoolEdge {
    /// Construct a `PoolEdge` from a full `Pool` object.
    pub fn from_pool(pool: &Pool) -> Self {
        let tvl = pool
            .tvl
            .filter(|tvl| tvl.is_finite() && *tvl >= 0.0)
            .unwrap_or(0.0);

        Self {
            pool_id: pool.metadata.id,
            pool_pubkey: pool.metadata.pubkey,
            protocol: pool.metadata.protocol,
            pool_type: pool.metadata.pool_type,
            tvl,
            fee_rate: pool.fee_rate,
        }
    }

    /// Returns a heuristic "cost" for this edge.
    ///
    /// Lower cost = better pool for routing.
    ///
    /// Formula: `fee_fraction + (1.0 / tvl)`
    /// - `fee_fraction`: fee_rate converted to a decimal (e.g. 2500 → 0.0025)
    /// - `1/tvl`: penalizes shallow pools — a $1M pool costs 0.000001,
    ///   while a $100 pool costs 0.01
    ///
    /// This is a static heuristic (doesn't depend on trade size).
    /// The actual math engine will compute exact slippage later.
    pub fn heuristic_cost(&self) -> f64 {
        let fee_fraction = self.fee_rate as f64 / 1_000_000.0;
        let tvl_penalty = if self.tvl > 0.0 {
            1.0 / self.tvl
        } else {
            f64::MAX
        };
        fee_fraction + tvl_penalty
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Raw route step data straight from the graph (no external lookups)
#[derive(Debug, Serialize)]
pub struct GraphRouteStep {
    pub pool_id: u32,
    pub pool_pubkey: PubkeyBytes,
    pub protocol: DexProtocol,
    pub pool_type: PoolType,
    pub fee_rate: u32,
    pub tvl: f64,
    pub input_mint: PubkeyBytes,
    pub output_mint: PubkeyBytes,
}

#[derive(Debug, Serialize)]
pub struct GraphRoute {
    /// Number of hops in this route
    pub hops: usize,
    /// Heuristic cost (sum of edge costs — lower is better)
    pub estimated_cost: f64,
    /// The swap steps
    pub steps: Vec<GraphRouteStep>,
}

#[derive(Debug, Serialize)]
pub struct RoutesResponse {
    pub input_mint: String,
    pub output_mint: String,
    pub routes_found: usize,
    pub routes: Vec<GraphRoute>,
}

#[derive(Debug, Deserialize)]
pub struct RouteQuery {
    pub input_mint: String,
    pub output_mint: String,
    pub amount: u64,
}

#[derive(Debug, Serialize)]
pub struct RankedRoute {
    pub amount_in: u64,
    pub estimated_amount_out: u64,
    pub route: GraphRoute,
}

#[derive(Debug, Serialize)]
pub struct RouteResponse {
    pub input_mint: String,
    pub output_mint: String,
    pub amount_in: u64,
    pub best_routes: Vec<RankedRoute>,
}
