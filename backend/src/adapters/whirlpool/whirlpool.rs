use crate::adapters::DexAdapter;
use crate::models::pool::{
    ClmmState, DexProtocol, Pool, PoolData, PoolId, PoolMetadata, PoolStatus, PoolType, PubkeyBytes,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const ORCA_API_URL: &str = "https://api.orca.so/v2/solana/pools";
const WHIRLPOOL_PROGRAM_ID: &str = "whirLb148YgbbHzbyen56UNeeY1rPMZZdg8Vw7K27aac";

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
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    decimals: u8,
}

pub struct WhirlpoolAdapter {
    program_id: Pubkey,
}

impl WhirlpoolAdapter {
    /// Returns Result<Self> to prevent unwrap() panics at startup.
    pub fn new() -> Result<Self> {
        let program_id = Pubkey::from_str(WHIRLPOOL_PROGRAM_ID)
            .context("Failed to parse static Whirlpool program ID")?;

        Ok(Self { program_id })
    }

    fn parse_pubkey(address: &str) -> Result<PubkeyBytes> {
        let pubkey = Pubkey::from_str(address).context("Invalid pubkey string")?;
        Ok(pubkey.to_bytes())
    }

    // Todo
    /// Derives the tick array PDA.
    /// Note: While manual derivation works for standard FixedTickArrays, utilizing Orca's
    /// `orca_whirlpools_core` SDK is recommended for production to safely handle their
    /// newer Dynamic Tick Arrays and format changes gracefully.
    fn derive_tick_array_pda(&self, whirlpool: &Pubkey, tick_start_index: i32) -> PubkeyBytes {
        let tick_string = tick_start_index.to_string();
        let (pda, _) = Pubkey::find_program_address(
            &[b"tick_array", whirlpool.as_ref(), tick_string.as_bytes()],
            &self.program_id,
        );
        pda.to_bytes()
    }

    /// Uses div_euclid for elegant, mathematically correct floor division on negative ticks.
    fn get_start_tick_index(&self, tick_index: i32, tick_spacing: u16) -> i32 {
        let ticks_per_array = 88 * tick_spacing as i32;
        tick_index.div_euclid(ticks_per_array) * ticks_per_array
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
        current_slot: u64,
    ) -> Result<Vec<Pool>> {
        let response = client
            .get(ORCA_API_URL)
            .send()
            .await?
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

            let liquidity = u128::from_str(&orca_pool.liquidity).unwrap_or_else(|e| {
                log::warn!("Invalid liquidity string for {}: {}", orca_pool.address, e);
                0
            });
            let sqrt_price_x64 = u128::from_str(&orca_pool.sqrt_price).unwrap_or_else(|e| {
                log::warn!("Invalid sqrt_price string for {}: {}", orca_pool.address, e);
                0
            });

            if liquidity == 0 {
                continue;
            }

            // Storing the Fee rate raw
            let fee_rate = orca_pool.fee_rate as u32;

            let current_start_tick =
                self.get_start_tick_index(orca_pool.tick_current_index, orca_pool.tick_spacing);
            let ticks_per_array = 88 * orca_pool.tick_spacing as i32;

            // Pre-calculate a sliding window of adjacent tick arrays (prev, current, next).
            // This satisfies SwapV2 look-ahead requirements for standard executions.
            let active_tick_array =
                self.derive_tick_array_pda(&pool_pubkey_raw, current_start_tick);
            let prev_tick_array =
                self.derive_tick_array_pda(&pool_pubkey_raw, current_start_tick - ticks_per_array);
            let next_tick_array =
                self.derive_tick_array_pda(&pool_pubkey_raw, current_start_tick + ticks_per_array);

            let metadata = PoolMetadata {
                id: *next_id,
                pubkey: pool_pubkey_raw.to_bytes(),
                protocol: DexProtocol::Whirlpool,
                pool_type: PoolType::Clmm,
                status: PoolStatus::Active,
                token_a_mint,
                token_b_mint,
                token_a_decimals: orca_pool.token_a.decimals,
                token_b_decimals: orca_pool.token_b.decimals,
                token_a_vault,
                token_b_vault,
            };

            let clmm_state = ClmmState {
                liquidity,
                sqrt_price_x64,
                current_tick_index: orca_pool.tick_current_index,
                tick_spacing: orca_pool.tick_spacing,
                // Populating a buffer of adjacent arrays ensures quoting and pathfinding won't panic
                tick_array_pubkeys: vec![prev_tick_array, active_tick_array, next_tick_array],
            };

            pools.push(Pool {
                metadata,
                data: PoolData::Clmm(clmm_state),
                fee_rate,
                last_updated_slot: current_slot,
            });

            *next_id += 1;
        }

        Ok(pools)
    }
}
