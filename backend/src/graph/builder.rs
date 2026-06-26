use crate::models::graph::PoolEdge;
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

/// Maximum number of pools to keep per token pair (sorted by TVL desc).
/// Keeps the graph lean — more than 5 parallel pools between the same
/// pair rarely adds routing value.
const TOP_N_PER_PAIR: usize = 5;

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
    /// Sum of heuristic costs across all steps (lower = better)
    pub total_cost: f64,
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

/// A directed multigraph representing the DEX ecosystem.
/// - **Nodes**: Token Mints (`PubkeyBytes`)
/// - **Edges**: Liquidity Pools (`PoolEdge`) with rich metadata
///
/// The `PoolEdge` on each edge carries fee_rate, tvl, protocol, pool_type,
/// and decimals — enough for heuristic scoring without external lookups.
pub type DexGraph = DiGraph<PubkeyBytes, PoolEdge>;

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
    /// Applies a multi-stage pruning pipeline:
    /// 1. **Status filter**: Only `Active` pools pass
    /// 2. **TVL threshold**: Pools below `min_tvl` are dropped (NaN-safe)
    /// 3. **Zero-mint filter**: Uninitialized or self-referencing pools are dropped
    /// 4. **Top-N per pair**: For each (mintA, mintB), only the top 5 pools by TVL
    ///    are inserted (the rest are discarded)
    /// 5. **Deduplication**: Same (src, dst, pool_id) edge is never added twice
    pub fn build_from_pools(&mut self, pools: &[Pool], min_tvl: f64) {
        // Clamp min_tvl: if caller passes NaN, treat it as 0.0 (accept everything).
        let safe_min_tvl = if min_tvl.is_finite() { min_tvl } else { 0.0 };

        // ── Stage 1-3: Filter pools ─────────────────────────────────────
        let mut valid_pools: Vec<&Pool> = pools
            .iter()
            .filter(|pool| {
                // Skip inactive pools
                if pool.metadata.status != PoolStatus::Active {
                    return false;
                }

                // NaN-safe TVL check
                let tvl = pool.tvl.unwrap_or(0.0);
                let tvl = if tvl.is_finite() { tvl } else { 0.0 };
                if tvl < safe_min_tvl {
                    return false;
                }

                let mint_a = pool.metadata.token_a.mint;
                let mint_b = pool.metadata.token_b.mint;

                // Skip pools with zero/uninitialized mints
                if mint_a == ZERO_MINT || mint_b == ZERO_MINT {
                    return false;
                }

                // Skip self-referencing pools (same token on both sides)
                if mint_a == mint_b {
                    return false;
                }

                true
            })
            .collect();

        // ── Stage 4: Top-N per pair pruning ─────────────────────────────
        // Sort all valid pools by TVL descending so that when we insert,
        // the highest-TVL pools are processed first.
        valid_pools.sort_by(|a, b| {
            let tvl_a = a.tvl.unwrap_or(0.0);
            let tvl_b = b.tvl.unwrap_or(0.0);
            tvl_b.partial_cmp(&tvl_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Track how many pools we've inserted per canonical pair
        let mut pair_counts: HashMap<(PubkeyBytes, PubkeyBytes), usize> = HashMap::new();

        for pool in valid_pools {
            let mint_a = pool.metadata.token_a.mint;
            let mint_b = pool.metadata.token_b.mint;

            // Canonical pair key (sorted order) to treat A-B and B-A as the same pair
            let pair_key = if mint_a < mint_b {
                (mint_a, mint_b)
            } else {
                (mint_b, mint_a)
            };

            // Check top-N limit for this pair
            let count = pair_counts.entry(pair_key).or_insert(0);
            if *count >= TOP_N_PER_PAIR {
                continue; // Already have enough pools for this pair
            }

            let node_a = self.get_or_add_node(mint_a);
            let node_b = self.get_or_add_node(mint_b);
            let pool_id = pool.metadata.id;

            // Build the rich edge weight
            let edge = PoolEdge::from_pool(pool);

            // ── Stage 5: Deduplication ──────────────────────────────────
            let mut inserted = false;
            if self.inserted_edges.insert((node_a, node_b, pool_id)) {
                self.graph.add_edge(node_a, node_b, edge.clone());
                inserted = true;
            }
            if self.inserted_edges.insert((node_b, node_a, pool_id)) {
                self.graph.add_edge(node_b, node_a, edge);
                inserted = true;
            }

            if inserted {
                *count += 1;
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
    /// # Returns
    /// Routes sorted by `total_cost` ascending (best routes first).
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
            let expanded = self.expand_node_path_to_routes(&node_path);

            for candidate in expanded {
                let fingerprint: Vec<PoolId> = candidate.steps.iter().map(|s| s.pool_id).collect();
                if seen_routes.insert(fingerprint) {
                    all_routes.push(candidate);
                }
            }
        }

        if all_routes.is_empty() {
            return None;
        }

        // Sort by heuristic cost ascending (best routes first)
        all_routes.sort_by(|a, b| {
            a.total_cost
                .partial_cmp(&b.total_cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Truncate to the requested limit
        all_routes.truncate(clamped_routes);

        Some(all_routes)
    }

    /// Takes a node path like [SOL, RAY, USDC] and expands it into concrete
    /// `RouteCandidate`s by enumerating all pool (edge) combinations per hop.
    ///
    /// Each candidate is scored by summing `heuristic_cost()` across its edges.
    /// Deduplicates pool_ids per hop to avoid duplicate edges producing duplicate routes.
    fn expand_node_path_to_routes(
        &self,
        node_path: &[NodeIndex],
    ) -> Vec<RouteCandidate> {
        // Each candidate is (steps, accumulated_cost)
        let mut candidates: Vec<(Vec<RouteStep>, f64)> = vec![(vec![], 0.0)];

        for window in node_path.windows(2) {
            let u = window[0];
            let v = window[1];

            let input_mint = self.graph[u];
            let output_mint = self.graph[v];

            // Collect UNIQUE edges for this u→v hop.
            // Deduplicate by pool_id to prevent duplicate graph edges
            // from producing duplicate route candidates.
            let mut seen_pool_ids = HashSet::new();
            let edges: Vec<(PoolId, f64)> = self
                .graph
                .edges(u)
                .filter(|e| e.target() == v)
                .filter(|e| seen_pool_ids.insert(e.weight().pool_id))
                .map(|e| (e.weight().pool_id, e.weight().heuristic_cost()))
                .collect();

            // Cartesian product: each existing candidate × each pool at this hop
            let mut next_candidates = Vec::new();
            for (candidate, cost) in &candidates {
                for &(pool_id, edge_cost) in &edges {
                    let mut new_steps = candidate.clone();
                    new_steps.push(RouteStep {
                        pool_id,
                        input_mint,
                        output_mint,
                    });
                    next_candidates.push((new_steps, cost + edge_cost));
                }
            }
            candidates = next_candidates;
        }

        candidates
            .into_iter()
            .map(|(steps, total_cost)| RouteCandidate { steps, total_cost })
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

    /// Returns a reference to a `PoolEdge` by pool_id, searching the graph edges.
    /// Used by the API handler to read pool metadata from the graph directly.
    pub fn get_pool_edge(&self, pool_id: PoolId) -> Option<&PoolEdge> {
        self.graph
            .edge_weights()
            .find(|edge| edge.pool_id == pool_id)
    }
}
