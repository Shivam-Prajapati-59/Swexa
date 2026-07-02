use crate::adapters::DexAdapter;
use crate::hydration::pubkey_to_bytes;
use crate::models::pool::{
    ClmmState, DexProtocol, Pool, PoolData, PoolId, PoolMetadata, PoolStatus, PoolToken, PoolType,
    PubkeyBytes,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const ORCA_API_URL: &str = "https://api.orca.so/v2/solana/pools";
const MIN_TVL_USD: f64 = 1000.0;

#[derive(Debug, Deserialize)]
struct OrcaApiResponse {
    data: Vec<OrcaPoolData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrcaPoolData {
    address: String,
    tick_spacing: u16,
    fee_rate: u32,
    liquidity: String,
    sqrt_price: String,
    tick_current_index: i32,
    token_mint_a: String,
    token_vault_a: String,
    token_mint_b: String,
    token_vault_b: String,
    token_a: TokenInfo,
    token_b: TokenInfo,
    tvl_usdc: String,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    decimals: u8,
    #[serde(default)]
    name: String,
    #[serde(default)]
    symbol: String,
}

#[derive(Default)]
pub struct WhirlpoolAdapter;

impl WhirlpoolAdapter {
    fn parse_pubkey(address: &str) -> Result<PubkeyBytes> {
        let pubkey = Pubkey::from_str(address).context("Invalid pubkey string")?;
        Ok(PubkeyBytes(pubkey.to_bytes()))
    }
}

impl DexAdapter for WhirlpoolAdapter {
    fn protocol_name(&self) -> &'static str {
        "Whirlpool"
    }

    async fn fetch_pools(
        &self,
        client: &Client,
        next_id: &mut PoolId,
        current_slot: Option<u64>,
    ) -> Result<Vec<Pool>> {
        use std::time::Duration;

        const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

        let response = client
            .get(ORCA_API_URL)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?
            .error_for_status()?
            .json::<OrcaApiResponse>()
            .await?;

        let mut pools = Vec::with_capacity(response.data.len());

        for orca_pool in response.data {
            let pool_pubkey_raw = match Pubkey::from_str(&orca_pool.address) {
                Ok(pk) => pk,
                Err(e) => {
                    log::warn!(
                        "WhirlpoolAdapter: Failed to parse pool address {}: {}",
                        orca_pool.address,
                        e
                    );
                    continue;
                }
            };

            let token_a_mint = match Self::parse_pubkey(&orca_pool.token_mint_a) {
                Ok(pk) => pk,
                Err(e) => {
                    log::warn!("Invalid token_a_mint for {}: {}", orca_pool.address, e);
                    continue;
                }
            };
            let token_b_mint = match Self::parse_pubkey(&orca_pool.token_mint_b) {
                Ok(pk) => pk,
                Err(e) => {
                    log::warn!("Invalid token_b_mint for {}: {}", orca_pool.address, e);
                    continue;
                }
            };
            let token_a_vault = match Self::parse_pubkey(&orca_pool.token_vault_a) {
                Ok(pk) => pk,
                Err(e) => {
                    log::warn!("Invalid vault A for {}: {}", orca_pool.address, e);
                    continue;
                }
            };
            let token_b_vault = match Self::parse_pubkey(&orca_pool.token_vault_b) {
                Ok(pk) => pk,
                Err(e) => {
                    log::warn!("Invalid vault B for {}: {}", orca_pool.address, e);
                    continue;
                }
            };

            let liquidity = match u128::from_str(&orca_pool.liquidity) {
                Ok(liquidity) => liquidity,
                Err(e) => {
                    log::warn!("Invalid liquidity string for {}: {}", orca_pool.address, e);
                    continue;
                }
            };
            let sqrt_price_x64 = match u128::from_str(&orca_pool.sqrt_price) {
                Ok(sqrt_price_x64) => sqrt_price_x64,
                Err(e) => {
                    log::warn!("Invalid sqrt_price string for {}: {}", orca_pool.address, e);
                    continue;
                }
            };
            if liquidity == 0 {
                continue;
            }

            let metadata = PoolMetadata {
                id: *next_id,
                pubkey: pubkey_to_bytes(pool_pubkey_raw),
                protocol: DexProtocol::Whirlpool,
                pool_type: PoolType::Clmm,
                status: PoolStatus::Active,
                token_a: PoolToken {
                    mint: token_a_mint,
                    name: orca_pool.token_a.name.clone(),
                    symbol: orca_pool.token_a.symbol.clone(),
                    decimals: orca_pool.token_a.decimals,
                    vault: Some(token_a_vault),
                },
                token_b: PoolToken {
                    mint: token_b_mint,
                    name: orca_pool.token_b.name.clone(),
                    symbol: orca_pool.token_b.symbol.clone(),
                    decimals: orca_pool.token_b.decimals,
                    vault: Some(token_b_vault),
                },
            };

            let clmm_state = ClmmState {
                liquidity: Some(liquidity),
                sqrt_price_x64: Some(sqrt_price_x64),
                current_tick_index: Some(orca_pool.tick_current_index),
                tick_spacing: orca_pool.tick_spacing,
                reserve_a: None,
                reserve_b: None,
                initialized_ticks: Vec::new(),
            };

            let tvl_usdc = match f64::from_str(&orca_pool.tvl_usdc) {
                Ok(tvl) => tvl,
                Err(e) => {
                    log::warn!("Invalid tvl_usdc for {}: {}", orca_pool.address, e);
                    continue;
                }
            };

            if tvl_usdc < MIN_TVL_USD {
                continue;
            }

            pools.push(Pool {
                metadata,
                data: PoolData::Clmm(clmm_state),
                fee_rate: orca_pool.fee_rate,
                tvl: Some(tvl_usdc),
                last_updated_slot: current_slot,
            });

            *next_id += 1;
        }

        Ok(pools)
    }
}
