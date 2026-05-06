use crate::routing::{GraphStats, GraphValidationReport, RouteGraph};
use crate::types::PoolEdge;

pub struct AppState {
    pub pools: Vec<PoolEdge>,
    pub graph: RouteGraph,
    pub stats: GraphStats,
    pub validation: GraphValidationReport,
}

impl AppState {
    pub fn from_pools(pools: Vec<PoolEdge>) -> Self {
        let graph = RouteGraph::new(&pools);
        let stats = graph.stats();
        let validation = graph.validate();

        Self {
            pools,
            graph,
            stats,
            validation,
        }
    }
}
