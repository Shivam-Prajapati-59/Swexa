use crate::graph::builder::GraphBuilder;
use crate::models::pool::Pool;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

type GraphCache = Option<(Arc<GraphBuilder>, u64)>;

#[derive(Clone)]
pub struct AppState {
    pub pools: Arc<RwLock<Vec<Pool>>>,
    /// Incremented every time pools are refreshed, invalidating the cached graph.
    pub pool_generation: Arc<AtomicU64>,
    /// Cached graph built from pools. Rebuilt lazily when generation changes.
    graph_cache: Arc<RwLock<GraphCache>>,
}

impl AppState {
    pub async fn new() -> Self {
        Self {
            pools: Arc::new(RwLock::new(Vec::new())),
            pool_generation: Arc::new(AtomicU64::new(0)),
            graph_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Returns a cached `GraphBuilder`, rebuilding it only when pools have changed.
    pub async fn get_or_build_graph(&self) -> Arc<GraphBuilder> {
        let generation = self.pool_generation.load(Ordering::Acquire);

        // Fast path: cached graph is still fresh
        {
            let cache = self.graph_cache.read().await;
            if let Some((ref builder, cached_gen)) = *cache
                && cached_gen == generation
            {
                return builder.clone();
            }
        }

        // Slow path: rebuild the graph
        let pools = self.pools.read().await;
        let mut builder = GraphBuilder::new();
        builder.build_from_pools(&pools, 0.0);

        let mut cache = self.graph_cache.write().await;
        let current_gen = self.pool_generation.load(Ordering::Acquire);

        // Double-check to avoid redundant rebuilds from concurrent requests
        if let Some((ref existing, cached_gen)) = *cache
            && cached_gen == current_gen
        {
            return existing.clone();
        }

        let shared = Arc::new(builder);
        *cache = Some((shared.clone(), current_gen));
        shared
    }
}
