use crate::models::pool::Pool;
use anyhow::Result;
use reqwest::Client;

pub mod whirlpool;
pub mod metora;
pub mod raydium;

/// The common interface for all DEX adapters.
/// 
/// Since we are using Rust 2024 edition, we can use native async traits.
#[allow(async_fn_in_trait)]
pub trait DexAdapter {
    /// Returns the protocol name (e.g., "Whirlpool").
    fn protocol_name(&self) -> &'static str;

    /// Fetches all pools from the DEX's HTTP API and converts them 
    /// into our normalized `Pool` structure.
    async fn fetch_pools(&self, client: &Client, next_id: &mut u32, current_slot: u64) -> Result<Vec<Pool>>;
}
