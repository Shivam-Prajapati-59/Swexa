use crate::adapters;
use crate::models::pool::Pool;
use crate::types::AppState;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Serialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::atomic::Ordering;

pub const DEFAULT_POOL_PAGE_SIZE: usize = 100;
pub const MAX_POOL_PAGE_SIZE: usize = 1_000;

#[derive(Debug, Serialize)]
pub struct PoolPage {
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub data: Vec<Pool>,
}

pub async fn refresh_pool_cache(state: &AppState) -> Result<Vec<Pool>> {
    let client = Client::new();
    let current_slot = current_slot().await;

    // Pool refresh is intentionally metadata-only. Quote-grade vault/tick/bin
    // hydration happens later for the small set of candidate route pools.
    let pools = adapters::fetch_all_pools(&client, current_slot)
        .await
        .context("failed to fetch DEX pools")?;

    let mut cached_pools = state.pools.write().await;
    *cached_pools = pools.clone();
    state.pool_generation.fetch_add(1, Ordering::Release);

    Ok(pools)
}

pub async fn cached_pool_page(state: &AppState, page: usize, page_size: usize) -> PoolPage {
    let pools = state.pools.read().await;
    paginate_pools(&pools, page, page_size)
}

pub fn paginate_pools(pools: &[Pool], page: usize, page_size: usize) -> PoolPage {
    let page = page.max(1);
    let page_size = page_size.clamp(1, MAX_POOL_PAGE_SIZE);
    let start = page.saturating_sub(1).saturating_mul(page_size);
    let data = pools.iter().skip(start).take(page_size).cloned().collect();

    PoolPage {
        total: pools.len(),
        page,
        page_size,
        data,
    }
}

async fn current_slot() -> Option<u64> {
    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
    RpcClient::new(rpc_url).get_slot().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pool::{
        CpmmState, DexProtocol, PoolData, PoolMetadata, PoolStatus, PoolToken, PoolType,
        PubkeyBytes,
    };

    fn pool(id: u32) -> Pool {
        Pool {
            metadata: PoolMetadata {
                id,
                pubkey: PubkeyBytes([id as u8; 32]),
                protocol: DexProtocol::Raydium,
                pool_type: PoolType::Cpmm,
                status: PoolStatus::Active,
                token_a: PoolToken {
                    mint: PubkeyBytes([1; 32]),
                    name: "A".to_string(),
                    symbol: "A".to_string(),
                    decimals: 6,
                    vault: None,
                },
                token_b: PoolToken {
                    mint: PubkeyBytes([2; 32]),
                    name: "B".to_string(),
                    symbol: "B".to_string(),
                    decimals: 6,
                    vault: None,
                },
            },
            data: PoolData::Cpmm(CpmmState {
                reserve_a: 1_000,
                reserve_b: 1_000,
            }),
            fee_rate: 0,
            tvl: Some(1_000.0),
            last_updated_slot: None,
        }
    }

    #[test]
    fn paginate_pools_clamps_and_slices() {
        let pools = vec![pool(1), pool(2), pool(3)];
        let page = paginate_pools(&pools, 2, 2);

        assert_eq!(page.total, 3);
        assert_eq!(page.page, 2);
        assert_eq!(page.page_size, 2);
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].metadata.id, 3);
    }
}
