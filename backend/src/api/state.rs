use crate::simulation::simulated_quote::SimulatedQuoteEngine;
use crate::routing::{GraphStats, GraphValidationReport, RouteGraph};
use crate::types::PoolEdge;

/// Default Solana mainnet RPC endpoint.
const DEFAULT_RPC_URL: &str = "https://api.mainnet-beta.solana.com";

pub struct AppState {
    pub pools: Vec<PoolEdge>,
    pub graph: RouteGraph,
    pub stats: GraphStats,
    pub validation: GraphValidationReport,
    /// The SVM-backed quote engine for simulation-based routing.
    pub simulated_engine: SimulatedQuoteEngine,
}

impl AppState {
    pub fn from_pools(pools: Vec<PoolEdge>) -> Self {
        Self::from_pools_with_rpc(pools, DEFAULT_RPC_URL)
    }

    pub fn from_pools_with_rpc(pools: Vec<PoolEdge>, rpc_url: &str) -> Self {
        let graph = RouteGraph::new(&pools);
        let stats = graph.stats();
        let validation = graph.validate();
        let simulated_engine = SimulatedQuoteEngine::new(rpc_url);

        Self {
            pools,
            graph,
            stats,
            validation,
            simulated_engine,
        }
    }
}
