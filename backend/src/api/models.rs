use crate::engine::quote::RouteQuote;
use crate::engine::simulated_quote::{SimulatedRouteQuote, SplitResult};
use crate::routing::{GraphStats, GraphValidationReport, MAX_SUPPORTED_HOPS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub pool_count: usize,
    pub max_supported_hops: usize,
    pub graph_ready: bool,
}

#[derive(Serialize)]
pub struct GraphStatsResponse {
    pub graph: GraphStats,
    pub validation: GraphValidationReport,
    pub dex_pool_counts: BTreeMap<&'static str, usize>,
    pub pool_type_counts: BTreeMap<&'static str, usize>,
}

#[derive(Debug, Deserialize)]
pub struct RouteQuery {
    pub source_mint: String,
    pub target_mint: String,
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    pub exact_hops: Option<usize>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize)]
pub struct QuoteQuery {
    pub source_mint: String,
    pub target_mint: String,
    pub amount_in: u64,
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    pub exact_hops: Option<usize>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// If `true`, forces the heuristic-only path (skips simulation).
    #[serde(default)]
    pub heuristic_only: bool,
}

#[derive(Serialize)]
pub struct RoutesResponse {
    pub source_mint: String,
    pub target_mint: String,
    pub requested_max_hops: usize,
    pub effective_max_hops: usize,
    pub returned_route_count: usize,
    pub routes: Vec<RouteSummary>,
}

/// Unified quote response that works for both heuristic and simulated modes.
#[derive(Serialize)]
pub struct QuoteResponse {
    pub quote_method: &'static str,
    pub source_mint: String,
    pub target_mint: String,
    pub amount_in: u64,
    pub requested_max_hops: usize,
    pub effective_max_hops: usize,
    pub candidate_route_count: usize,
    pub best_path_index: usize,
    pub best_quote: RouteQuote,
    pub best_path: RouteSummary,
    pub quoted_routes: Vec<QuotedRouteSummary>,
}

/// Extended quote response when using the SVM simulator.
#[derive(Serialize)]
pub struct SimulatedQuoteResponse {
    pub quote_method: &'static str,
    pub source_mint: String,
    pub target_mint: String,
    pub amount_in: u64,
    pub requested_max_hops: usize,
    pub effective_max_hops: usize,
    pub candidate_route_count: usize,
    /// The best single route or split.
    pub best: SimulatedRouteQuote,
    /// The best route's path summary.
    pub best_path: RouteSummary,
    /// All individual route simulations.
    pub all_quotes: Vec<SimulatedQuotedRouteSummary>,
    /// If splitting improved the result, details are here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitResult>,
}

#[derive(Serialize)]
pub struct SimulatedQuotedRouteSummary {
    pub route_index: usize,
    pub amount_out: u64,
    pub simulated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub route: RouteSummary,
}

#[derive(Serialize)]
pub struct QuotedRouteSummary {
    pub route_index: usize,
    pub quote: RouteQuote,
    pub route: RouteSummary,
}

#[derive(Serialize)]
pub struct RouteSummary {
    pub hops: usize,
    pub total_fee_rate: f64,
    pub estimated_total_fee_bps: u64,
    pub path: Vec<RouteHop>,
}

#[derive(Serialize)]
pub struct RouteHop {
    pub pool_address: String,
    pub dex: &'static str,
    pub pool_type: &'static str,
    pub from_mint: String,
    pub from_symbol: String,
    pub to_mint: String,
    pub to_symbol: String,
    pub fee_rate: f64,
    pub tvl: f64,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

fn default_max_hops() -> usize {
    MAX_SUPPORTED_HOPS
}

fn default_limit() -> usize {
    20
}
