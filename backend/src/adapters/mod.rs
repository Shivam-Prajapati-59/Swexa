use crate::models::pool::Pool;
use anyhow::{Context, Result};
use reqwest::Client;

pub mod meteora;
pub mod raydium;
pub mod whirlpool;

use meteora::MeteoraAdapter;
use raydium::RaydiumAdapter;
use whirlpool::WhirlpoolAdapter;

/// The common interface for all DEX adapters.
///
/// Since we are using Rust 2024 edition, we can use native async traits.
#[allow(async_fn_in_trait)]
pub trait DexAdapter {
    /// Returns the protocol name (e.g., "Whirlpool").
    fn protocol_name(&self) -> &'static str;

    /// Fetches all pools from the DEX's HTTP API and converts them
    /// into our normalized `Pool` structure.
    async fn fetch_pools(
        &self,
        client: &Client,
        next_id: &mut u32,
        current_slot: Option<u64>,
    ) -> Result<Vec<Pool>>;
}

pub async fn fetch_all_pools(client: &Client, current_slot: Option<u64>) -> Result<Vec<Pool>> {
    let mut next_id = 0;
    let mut all_pools = Vec::new();

    let raydium = RaydiumAdapter::new();
    let mut raydium_pools = raydium
        .fetch_pools(client, &mut next_id, current_slot)
        .await
        .context("failed to fetch Raydium pools")?;
    all_pools.append(&mut raydium_pools);

    let meteora = MeteoraAdapter::new();
    let mut meteora_pools = meteora
        .fetch_pools(client, &mut next_id, current_slot)
        .await
        .context("failed to fetch Meteora pools")?;
    all_pools.append(&mut meteora_pools);

    let whirlpool = WhirlpoolAdapter::new();
    let mut whirlpool_pools = whirlpool
        .fetch_pools(client, &mut next_id, current_slot)
        .await
        .context("failed to fetch Whirlpool pools")?;
    all_pools.append(&mut whirlpool_pools);

    Ok(all_pools)
}
