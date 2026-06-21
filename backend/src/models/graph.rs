use crate::models::pool::PubkeyBytes;
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
// Response types
// ---------------------------------------------------------------------------

/// Enriched route step with human-readable pool and token info
#[derive(Debug, Serialize)]
pub struct EnrichedRouteStep {
    /// Pool address (Base58)
    pub pool_address: PubkeyBytes,
    /// Pool ID (internal)
    pub pool_id: u32,
    /// Protocol name (e.g. "Raydium", "Meteora", "Whirlpool")
    pub protocol: String,
    /// Input token info
    pub input: TokenSummary,
    /// Output token info
    pub output: TokenSummary,
}

#[derive(Debug, Serialize)]
pub struct TokenSummary {
    pub mint: PubkeyBytes,
    pub symbol: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct EnrichedRoute {
    /// Number of hops in this route
    pub hops: usize,
    /// The swap steps with full metadata
    pub steps: Vec<EnrichedRouteStep>,
}

#[derive(Debug, Serialize)]
pub struct RoutesResponse {
    pub input_mint: String,
    pub output_mint: String,
    pub routes_found: usize,
    pub routes: Vec<EnrichedRoute>,
}
