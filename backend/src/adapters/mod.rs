pub mod meteora;
pub mod raydium;
pub mod whirlpool;
// pub mod phoenix;

use crate::types::{DEFAULT_MIN_TVL, PoolEdge};

/// Fetches pools from **all** supported DEX APIs concurrently, converts them
/// into the common [`PoolEdge`] type, and returns a single merged vector.
///
/// Pools with TVL below `min_tvl` (default $1 000) are filtered out to keep
/// the routing graph manageable.
pub async fn fetch_all_pools(min_tvl: Option<f64>) -> Vec<PoolEdge> {
    let threshold = min_tvl.unwrap_or(DEFAULT_MIN_TVL);

    // Fire all API calls concurrently.
    let (whirlpool_result, raydium_result, meteora_result) = tokio::join!(
        whirlpool::fetch_whirlpools_api(),
        raydium::fetch_raydium_pools(),
        meteora::fetch_meteora_pools(threshold),
    );

    let mut edges: Vec<PoolEdge> = Vec::new();

    // ── Whirlpool ──────────────────────────────────────────────────────
    match whirlpool_result {
        Ok(pools) => {
            let before = pools.len();
            let converted: Vec<PoolEdge> = pools
                .into_iter()
                .map(PoolEdge::from)
                .filter(|e| e.tvl >= threshold)
                .collect();
            println!(
                "[Whirlpool] {} pools fetched, {} passed TVL filter (≥ ${:.0})",
                before,
                converted.len(),
                threshold
            );
            edges.extend(converted);
        }
        Err(err) => {
            eprintln!("[Whirlpool] Failed to fetch pools: {}", err);
        }
    }

    // ── Raydium ────────────────────────────────────────────────────────
    match raydium_result {
        Ok(pools) => {
            let before = pools.len();
            let converted: Vec<PoolEdge> = pools
                .into_iter()
                .map(PoolEdge::from)
                .filter(|e| e.tvl >= threshold)
                .collect();
            println!(
                "[Raydium]   {} pools fetched, {} passed TVL filter (≥ ${:.0})",
                before,
                converted.len(),
                threshold
            );
            edges.extend(converted);
        }
        Err(err) => {
            eprintln!("[Raydium]   Failed to fetch pools: {}", err);
        }
    }

    // ── Meteora ────────────────────────────────────────────────────────
    match meteora_result {
        Ok(pools) => {
            let before = pools.len();
            let converted: Vec<PoolEdge> = pools
                .into_iter()
                .map(PoolEdge::from)
                .filter(|e| e.tvl >= threshold)
                .collect();
            println!(
                "[Meteora]   {} pools fetched, {} passed TVL filter (≥ ${:.0})",
                before,
                converted.len(),
                threshold
            );
            edges.extend(converted);
        }
        Err(err) => {
            eprintln!("[Meteora]   Failed to fetch pools: {}", err);
        }
    }

    println!(
        "\n✅ Total pool edges available for routing: {}",
        edges.len()
    );
    edges
}
