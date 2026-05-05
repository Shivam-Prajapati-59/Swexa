use crate::types::PoolEdge;
use petgraph::algo::dijkstra;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{EdgeRef, Reversed};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

/// Tokens are Nodes, Pools are Edges.
pub struct RouteGraph {
    /// The petgraph directed graph. We use directed edges in both directions
    /// since AMM pools allow trading both ways.
    pub graph: DiGraph<String, PoolEdge>,
    /// Maps a token mint string to its `NodeIndex` in the graph for fast O(1) lookups.
    pub mint_to_node: HashMap<String, NodeIndex>,
    /// Pools ignored during graph construction because both sides point to the same mint.
    pub dropped_self_loop_pools: usize,
}

/// A path is a sequence of connected pools.
pub type Route = Vec<PoolEdge>;

/// The maximum hop count supported by Step 1 pathfinding.
pub const MAX_SUPPORTED_HOPS: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct GraphStats {
    pub token_count: usize,
    pub unique_pool_count: usize,
    pub directed_edge_count: usize,
    pub dropped_self_loop_pools: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphValidationReport {
    pub is_valid: bool,
    pub missing_node_mappings: usize,
    pub mismatched_node_weights: usize,
    pub stale_node_mappings: usize,
    pub invalid_edge_endpoints: usize,
    pub missing_reverse_edges: usize,
}

impl RouteGraph {
    /// Builds the `petgraph` structure from a flat list of pools.
    pub fn new(pools: &[PoolEdge]) -> Self {
        // Pre-allocate to reduce memory reallocations during graph construction
        let mut graph = DiGraph::<String, PoolEdge>::with_capacity(pools.len(), pools.len() * 2);
        let mut mint_to_node = HashMap::with_capacity(pools.len());
        let mut dropped_self_loop_pools = 0usize;

        for pool in pools {
            if pool.token_a.mint == pool.token_b.mint {
                dropped_self_loop_pools += 1;
                continue;
            }

            // Use the Entry API to safely get or insert Node A without borrowing errors
            let mint_a = pool.token_a.mint.clone();
            let node_a = *mint_to_node
                .entry(mint_a.clone())
                .or_insert_with(|| graph.add_node(mint_a));

            // Use the Entry API to safely get or insert Node B without borrowing errors
            let mint_b = pool.token_b.mint.clone();
            let node_b = *mint_to_node
                .entry(mint_b.clone())
                .or_insert_with(|| graph.add_node(mint_b));

            // AMM pools are bidirectional, so we add two directed edges for each pool
            graph.add_edge(node_a, node_b, pool.clone());
            graph.add_edge(node_b, node_a, pool.clone());
        }

        Self {
            graph,
            mint_to_node,
            dropped_self_loop_pools,
        }
    }

    pub fn stats(&self) -> GraphStats {
        let mut unique_pool_addresses = HashSet::with_capacity(self.graph.edge_count() / 2);

        for edge in self.graph.edge_weights() {
            unique_pool_addresses.insert(edge.address.clone());
        }

        GraphStats {
            token_count: self.graph.node_count(),
            unique_pool_count: unique_pool_addresses.len(),
            directed_edge_count: self.graph.edge_count(),
            dropped_self_loop_pools: self.dropped_self_loop_pools,
        }
    }

    pub fn validate(&self) -> GraphValidationReport {
        let mut missing_node_mappings = 0usize;
        let mut mismatched_node_weights = 0usize;
        let mut stale_node_mappings = 0usize;
        let mut invalid_edge_endpoints = 0usize;
        let mut missing_reverse_edges = 0usize;

        for node_index in self.graph.node_indices() {
            let mint = &self.graph[node_index];
            match self.mint_to_node.get(mint) {
                Some(mapped_index) if *mapped_index == node_index => {}
                Some(_) => mismatched_node_weights += 1,
                None => missing_node_mappings += 1,
            }
        }

        for (mint, node_index) in &self.mint_to_node {
            match self.graph.node_weight(*node_index) {
                Some(node_mint) if node_mint == mint => {}
                Some(_) | None => stale_node_mappings += 1,
            }
        }

        for edge in self.graph.edge_references() {
            let source = edge.source();
            let target = edge.target();
            let pool = edge.weight();
            let source_mint = &self.graph[source];
            let target_mint = &self.graph[target];

            let forward_matches =
                source_mint == &pool.token_a.mint && target_mint == &pool.token_b.mint;
            let reverse_matches =
                source_mint == &pool.token_b.mint && target_mint == &pool.token_a.mint;

            if !forward_matches && !reverse_matches {
                invalid_edge_endpoints += 1;
            }

            let has_reverse_edge = self.graph.edges(target).any(|reverse_edge| {
                reverse_edge.target() == source && reverse_edge.weight().address == pool.address
            });

            if !has_reverse_edge {
                missing_reverse_edges += 1;
            }
        }

        GraphValidationReport {
            is_valid: missing_node_mappings == 0
                && mismatched_node_weights == 0
                && stale_node_mappings == 0
                && invalid_edge_endpoints == 0
                && missing_reverse_edges == 0,
            missing_node_mappings,
            mismatched_node_weights,
            stale_node_mappings,
            invalid_edge_endpoints,
            missing_reverse_edges,
        }
    }

    /// Finds all possible routes between `source_mint` and `target_mint` up to `max_hops`.
    ///
    /// This uses Dijkstra on the reversed graph to compute the minimum remaining
    /// hop count to the target for every node. Those lower bounds let us prune
    /// branches aggressively while still returning all simple routes within the
    /// hop budget.
    pub fn find_routes(
        &self,
        source_mint: &str,
        target_mint: &str,
        max_hops: usize,
        limit: Option<usize>,
    ) -> Vec<Route> {
        let mut routes = Vec::new();
        let max_hops = max_hops.min(MAX_SUPPORTED_HOPS);
        let route_limit = limit.unwrap_or(usize::MAX);

        if max_hops == 0 || route_limit == 0 {
            return routes;
        }

        // Resolve start and target nodes; exit early if either mint is not in the graph
        let Some(&start_node) = self.mint_to_node.get(source_mint) else {
            return routes;
        };
        let Some(&target_node) = self.mint_to_node.get(target_mint) else {
            return routes;
        };

        let remaining_hops = dijkstra(Reversed(&self.graph), target_node, None, |_| 1usize);
        let Some(&start_distance) = remaining_hops.get(&start_node) else {
            return routes;
        };

        if start_distance > max_hops {
            return routes;
        }

        let mut stack = vec![SearchState {
            node: start_node,
            route: Vec::with_capacity(max_hops),
            visited: vec![start_node],
        }];

        while let Some(state) = stack.pop() {
            if routes.len() >= route_limit {
                break;
            }

            if state.node == target_node && !state.route.is_empty() {
                routes.push(state.route);
                continue;
            }

            let used_hops = state.route.len();
            if used_hops >= max_hops {
                continue;
            }

            let mut candidates = self
                .graph
                .edges(state.node)
                .filter_map(|edge| {
                    let next_node = edge.target();

                    if state.visited.contains(&next_node) {
                        return None;
                    }

                    let lower_bound = *remaining_hops.get(&next_node)?;
                    if used_hops + 1 + lower_bound > max_hops {
                        return None;
                    }

                    Some((edge.weight().clone(), next_node, lower_bound))
                })
                .collect::<Vec<_>>();

            candidates.sort_by_key(|(pool, _, lower_bound)| {
                (
                    *lower_bound,
                    Reverse(pool.tvl.to_bits()),
                    pool.fee_rate.to_bits(),
                    pool.address.clone(),
                )
            });

            for (pool, next_node, _) in candidates.into_iter().rev() {
                let mut next_route = state.route.clone();
                next_route.push(pool);

                let mut next_visited = state.visited.clone();
                next_visited.push(next_node);

                stack.push(SearchState {
                    node: next_node,
                    route: next_route,
                    visited: next_visited,
                });
            }
        }

        routes
    }
}

#[derive(Clone)]
struct SearchState {
    node: NodeIndex,
    route: Route,
    visited: Vec<NodeIndex>,
}

#[cfg(test)]
mod tests {
    use super::{MAX_SUPPORTED_HOPS, RouteGraph};
    use crate::types::{DexProtocol, PoolEdge, PoolType, TokenMint};

    fn token(mint: &str) -> TokenMint {
        TokenMint {
            mint: mint.to_string(),
            symbol: mint.to_string(),
            decimals: 9,
        }
    }

    fn pool(address: &str, a: &str, b: &str, tvl: f64) -> PoolEdge {
        PoolEdge {
            address: address.to_string(),
            dex: DexProtocol::Raydium,
            token_a: token(a),
            token_b: token(b),
            fee_rate: 0.003,
            tvl,
            pool_type: PoolType::Amm,
        }
    }

    #[test]
    fn graph_validation_passes_for_bidirectional_graph() {
        let pools = vec![pool("ab", "A", "B", 1_000.0), pool("bc", "B", "C", 2_000.0)];
        let graph = RouteGraph::new(&pools);

        let stats = graph.stats();
        let validation = graph.validate();

        assert_eq!(stats.token_count, 3);
        assert_eq!(stats.unique_pool_count, 2);
        assert_eq!(stats.directed_edge_count, 4);
        assert!(validation.is_valid);
        assert_eq!(validation.missing_reverse_edges, 0);
    }

    #[test]
    fn graph_validation_catches_invalid_edge_endpoint() {
        let pools = vec![pool("ab", "A", "B", 1_000.0)];
        let mut graph = RouteGraph::new(&pools);
        let a = graph.mint_to_node["A"];
        let b = graph.mint_to_node["B"];
        graph.graph.add_edge(a, b, pool("xy", "X", "Y", 1_000.0));

        let validation = graph.validate();

        assert!(!validation.is_valid);
        assert_eq!(validation.invalid_edge_endpoints, 1);
    }

    #[test]
    fn graph_drops_self_loop_pools() {
        let pools = vec![pool("aa", "A", "A", 1_000.0), pool("ab", "A", "B", 1_000.0)];
        let graph = RouteGraph::new(&pools);

        let stats = graph.stats();

        assert_eq!(stats.dropped_self_loop_pools, 1);
        assert_eq!(stats.unique_pool_count, 1);
        assert_eq!(stats.directed_edge_count, 2);
    }

    #[test]
    fn find_routes_supports_four_hops() {
        let pools = vec![
            pool("ab", "A", "B", 2_000.0),
            pool("bc", "B", "C", 2_000.0),
            pool("cd", "C", "D", 2_000.0),
            pool("de", "D", "E", 2_000.0),
            pool("ac", "A", "C", 10_000.0),
            pool("ce", "C", "E", 10_000.0),
        ];
        let graph = RouteGraph::new(&pools);

        let routes = graph.find_routes("A", "E", MAX_SUPPORTED_HOPS, None);

        assert!(routes.iter().any(|route| route.len() == 4));
        assert!(routes.iter().any(|route| route.len() == 2));
    }

    #[test]
    fn route_limit_is_respected() {
        let pools = vec![
            pool("ab", "A", "B", 2_000.0),
            pool("ac", "A", "C", 3_000.0),
            pool("bd", "B", "D", 2_000.0),
            pool("cd", "C", "D", 2_000.0),
            pool("de", "D", "E", 2_000.0),
            pool("be", "B", "E", 5_000.0),
            pool("ce", "C", "E", 5_000.0),
        ];
        let graph = RouteGraph::new(&pools);

        let routes = graph.find_routes("A", "E", MAX_SUPPORTED_HOPS, Some(2));

        assert_eq!(routes.len(), 2);
    }
}
