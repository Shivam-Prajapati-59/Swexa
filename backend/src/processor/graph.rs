use crate::types::PoolEdge;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, VecDeque};

/// Tokens are Nodes, Pools are Edges.
pub struct RouteGraph {
    /// The petgraph directed graph. We use directed edges in both directions
    /// since AMM pools allow trading both ways.
    pub graph: DiGraph<String, PoolEdge>,
    /// Maps a token mint string to its `NodeIndex` in the graph for fast O(1) lookups.
    pub mint_to_node: HashMap<String, NodeIndex>,
}

/// A path is a sequence of connected pools.
pub type Route = Vec<PoolEdge>;

impl RouteGraph {
    /// Builds the `petgraph` structure from a flat list of pools.
    pub fn new(pools: &[PoolEdge]) -> Self {
        // Pre-allocate to reduce memory reallocations during graph construction
        let mut graph = DiGraph::<String, PoolEdge>::with_capacity(pools.len(), pools.len() * 2);
        let mut mint_to_node = HashMap::with_capacity(pools.len());

        for pool in pools {
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
        }
    }

    /// Finds all possible routes between `source_mint` and `target_mint` up to `max_depth`.
    /// Uses Breadth-First Search (BFS) to find shortest paths first.
    pub fn find_routes(
        &self,
        source_mint: &str,
        target_mint: &str,
        max_depth: usize,
    ) -> Vec<Route> {
        let mut routes = Vec::new();

        // Resolve start and target nodes; exit early if either mint is not in the graph
        let Some(&start_node) = self.mint_to_node.get(source_mint) else {
            return routes;
        };
        let Some(&target_node) = self.mint_to_node.get(target_mint) else {
            return routes;
        };

        // Queue holds: (current_node_index, current_path_of_edges, visited_nodes)
        let mut queue: VecDeque<(NodeIndex, Route, Vec<NodeIndex>)> = VecDeque::new();

        // Pre-allocate the visited vector based on max_depth to prevent resizing
        let mut initial_visited = Vec::with_capacity(max_depth + 1);
        initial_visited.push(start_node);

        // Start the BFS queue with an empty route capacity-tuned to max_depth
        queue.push_back((start_node, Vec::with_capacity(max_depth), initial_visited));

        while let Some((current_node, current_path, visited)) = queue.pop_front() {
            // If we reached the target, save the route and skip further exploration from here
            if current_node == target_node && !current_path.is_empty() {
                routes.push(current_path);
                continue;
            }

            // Stop exploring this branch if we've hit the depth limit
            if current_path.len() >= max_depth {
                continue;
            }

            // Iterate over all outgoing edges from the current node
            for edge in self.graph.edges(current_node) {
                let next_node = edge.target();

                // Cycle prevention: only traverse if we haven't visited this node in the current path
                if !visited.contains(&next_node) {
                    // Clone the path and append the new pool edge
                    let mut next_path = current_path.clone();
                    next_path.push(edge.weight().clone());

                    // Clone the visited list and mark the next node as visited
                    let mut next_visited = visited.clone();
                    next_visited.push(next_node);

                    // Enqueue the new state for the next BFS layer
                    queue.push_back((next_node, next_path, next_visited));
                }
            }
        }

        routes
    }
}
