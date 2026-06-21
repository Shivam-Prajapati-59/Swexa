use crate::models::pool::{Pool, PoolId, PoolStatus, PubkeyBytes};
use petgraph::algo::all_simple_paths;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::hash::RandomState;

// ---------------------------------------------------------------------------
// Constants — hard limits to prevent OOM / CPU bombs
// ---------------------------------------------------------------------------

/// Absolute ceiling on max_hops to prevent exponential path explosion.
/// Even Jupiter caps at 4 hops for production routing.
const MAX_HOPS_CEILING: usize = 4;

/// Default route limit if caller doesn't specify one.
const DEFAULT_MAX_ROUTES: usize = 200;

// ---------------------------------------------------------------------------
// Route output type
// ---------------------------------------------------------------------------

/// A single candidate swap route discovered by the graph search.
///
/// Each `RouteStep` encodes both the pool to use AND the direction of the swap,
/// so the caller never needs to re-derive direction from pool metadata.
#[derive(Debug, Clone, Serialize)]
pub struct RouteCandidate {
    pub steps: Vec<RouteStep>,
}

/// One hop in a multi-hop swap route.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
pub struct RouteStep {
    /// The pool to swap through.
    pub pool_id: PoolId,
    /// The token mint going INTO this pool.
    pub input_mint: PubkeyBytes,
    /// The token mint coming OUT of this pool.
    pub output_mint: PubkeyBytes,
}

// ---------------------------------------------------------------------------
// Graph types
// ---------------------------------------------------------------------------

/// A directed graph representing the DEX ecosystem.
/// - Nodes are Token Mints (`PubkeyBytes`)
/// - Edges are Liquidity Pools (`PoolId`)
pub type DexGraph = DiGraph<PubkeyBytes, PoolId>;

/// The zero pubkey — used to filter broken/uninitialized pool mints.
const ZERO_MINT: PubkeyBytes = PubkeyBytes([0u8; 32]);

// ---------------------------------------------------------------------------
// GraphBuilder
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct GraphBuilder {
    /// The petgraph directed graph instance.
    pub graph: DexGraph,
    /// Maps a Token Mint → its NodeIndex. Prevents duplicate nodes.
    pub mint_to_node: HashMap<PubkeyBytes, NodeIndex>,
    /// Tracks (node_a, node_b, pool_id) to prevent duplicate edges
    /// when `build_from_pools` is called multiple times with overlapping data.
    inserted_edges: HashSet<(NodeIndex, NodeIndex, PoolId)>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gets the existing `NodeIndex` for a token mint,
    /// or creates a new node if it doesn't exist.
    pub fn get_or_add_node(&mut self, mint: PubkeyBytes) -> NodeIndex {
        *self
            .mint_to_node
            .entry(mint)
            .or_insert_with(|| self.graph.add_node(mint))
    }

    /// Populates the graph from discovered pools.
    ///
    /// Filters out:
    /// - Pools with `status != Active`
    /// - Pools with TVL below `min_tvl` (NaN-safe: NaN TVL is treated as 0.0)
    /// - Pools with zero-mint (uninitialized) token addresses
    /// - Duplicate edges from repeated calls
    pub fn build_from_pools(&mut self, pools: &[Pool], min_tvl: f64) {
        // Clamp min_tvl: if caller passes NaN, treat it as 0.0 (accept everything).
        let safe_min_tvl = if min_tvl.is_finite() { min_tvl } else { 0.0 };

        for pool in pools {
            // Skip inactive pools
            if pool.metadata.status != PoolStatus::Active {
                continue;
            }

            // NaN-safe TVL check
            let tvl = pool.tvl.unwrap_or(0.0);
            let tvl = if tvl.is_finite() { tvl } else { 0.0 };
            if tvl < safe_min_tvl {
                continue;
            }

            let mint_a = pool.metadata.token_a.mint;
            let mint_b = pool.metadata.token_b.mint;

            // Skip pools with zero/uninitialized mints
            if mint_a == ZERO_MINT || mint_b == ZERO_MINT {
                continue;
            }

            // Skip self-referencing pools (same token on both sides)
            if mint_a == mint_b {
                continue;
            }

            let node_a = self.get_or_add_node(mint_a);
            let node_b = self.get_or_add_node(mint_b);
            let pool_id = pool.metadata.id;

            // Deduplicate: only add edge if this exact (src, dst, pool_id) hasn't been seen
            if self.inserted_edges.insert((node_a, node_b, pool_id)) {
                self.graph.add_edge(node_a, node_b, pool_id);
            }
            if self.inserted_edges.insert((node_b, node_a, pool_id)) {
                self.graph.add_edge(node_b, node_a, pool_id);
            }
        }
    }

    /// Finds all possible swap routes between `input_mint` and `output_mint`.
    ///
    /// # Arguments
    /// - `max_hops`: Exact number of swaps (edges). Clamped to `MAX_HOPS_CEILING` (4).
    /// - `max_routes`: Maximum number of **distinct** `RouteCandidate`s to return.
    ///   0 means use `DEFAULT_MAX_ROUTES`.
    ///
    /// # How petgraph's `all_simple_paths` works
    /// The function takes `min_intermediate_nodes` and `max_intermediate_nodes` — these
    /// count **intermediate** nodes (excluding start and end), NOT edges.
    /// For `max_hops` edges, we need `max_hops - 1` intermediate nodes.
    /// Example: A→B→C is 2 hops (edges) and 1 intermediate node (B).
    pub fn find_all_routes(
        &self,
        input_mint: &PubkeyBytes,
        output_mint: &PubkeyBytes,
        max_hops: usize,
        max_routes: usize,
    ) -> Option<Vec<RouteCandidate>> {
        let start_node = *self.mint_to_node.get(input_mint)?;
        let end_node = *self.mint_to_node.get(output_mint)?;

        // Clamp max_hops to ceiling to prevent exponential explosion
        let clamped_hops = max_hops.clamp(1, MAX_HOPS_CEILING);

        // Clamp max_routes: 0 → default
        let clamped_routes = if max_routes == 0 {
            DEFAULT_MAX_ROUTES
        } else {
            max_routes
        };

        // Convert edge count → intermediate node count for petgraph
        // Both min and max are set to the same value so we get exactly N-hop routes
        let intermediate = clamped_hops.saturating_sub(1);

        let node_paths = all_simple_paths::<Vec<NodeIndex>, _, RandomState>(
            &self.graph,
            start_node,
            end_node,
            intermediate, // exact hop count
            Some(intermediate),
        );

        // Use a HashSet of pool-id-tuples to deduplicate routes
        let mut seen_routes: HashSet<Vec<PoolId>> = HashSet::new();
        let mut all_routes: Vec<RouteCandidate> = Vec::new();

        for node_path in node_paths {
            // Early exit if we've already collected enough distinct routes
            if all_routes.len() >= clamped_routes {
                break;
            }

            let remaining = clamped_routes - all_routes.len();
            let expanded = self.expand_node_path_to_routes(&node_path, remaining);

            for candidate in expanded {
                // Deduplicate: extract the pool_id sequence as a fingerprint
                let fingerprint: Vec<PoolId> = candidate.steps.iter().map(|s| s.pool_id).collect();
                if seen_routes.insert(fingerprint) {
                    all_routes.push(candidate);
                    if all_routes.len() >= clamped_routes {
                        break;
                    }
                }
            }
        }

        if all_routes.is_empty() {
            None
        } else {
            Some(all_routes)
        }
    }

    /// Takes a node path like [SOL, RAY, USDC] and expands it into concrete
    /// `RouteCandidate`s by enumerating all pool (edge) combinations per hop.
    ///
    /// Caps output at `limit` to prevent combinatorial explosion.
    /// Deduplicates pool_ids per hop to avoid duplicate edges producing duplicate routes.
    fn expand_node_path_to_routes(
        &self,
        node_path: &[NodeIndex],
        limit: usize,
    ) -> Vec<RouteCandidate> {
        // Start with one empty route
        let mut candidates: Vec<Vec<RouteStep>> = vec![vec![]];

        for window in node_path.windows(2) {
            let u = window[0];
            let v = window[1];

            let input_mint = self.graph[u];
            let output_mint = self.graph[v];

            // Collect UNIQUE pool IDs for this u→v edge
            // This prevents duplicate edges (from graph construction) from
            // producing duplicate route candidates.
            let mut seen_pool_ids = HashSet::new();
            let pool_ids: Vec<PoolId> = self
                .graph
                .edges(u)
                .filter(|e| e.target() == v)
                .map(|e| *e.weight())
                .filter(|id| seen_pool_ids.insert(*id))
                .collect();

            // Cartesian product: each existing candidate × each pool at this hop
            let mut next_candidates = Vec::new();
            for candidate in &candidates {
                for &pool_id in &pool_ids {
                    if next_candidates.len() >= limit {
                        break;
                    }
                    let mut new_candidate = candidate.clone();
                    new_candidate.push(RouteStep {
                        pool_id,
                        input_mint,
                        output_mint,
                    });
                    next_candidates.push(new_candidate);
                }
                if next_candidates.len() >= limit {
                    break;
                }
            }
            candidates = next_candidates;
        }

        candidates
            .into_iter()
            .map(|steps| RouteCandidate { steps })
            .collect()
    }

    /// Returns the number of unique token nodes in the graph.
    pub fn token_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Returns the number of directed edges (pool connections) in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}
