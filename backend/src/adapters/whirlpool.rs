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
// Standard Orca Whirlpool Program ID on Solana Mainnet
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
    fee_rate: u32, // Changed to u32 to safely catch raw values before conversion
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
    pub fn new() -> Self {
        Self {
            program_id: Pubkey::from_str(WHIRLPOOL_PROGRAM_ID).unwrap(),
        }
    }

    fn parse_pubkey(address: &str) -> Result<PubkeyBytes> {
        let pubkey = Pubkey::from_str(address).context("Invalid pubkey string")?;
        Ok(pubkey.to_bytes())
    }

    /// Programmatically derives the PDA of a Whirlpool Tick Array account.
    /// Essential for matching engines to bridge look-ahead logic cross-tick swaps.
    fn derive_tick_array_pda(&self, whirlpool: &Pubkey, tick_start_index: i32) -> PubkeyBytes {
        let (pda, _) = Pubkey::find_program_address(
            &[
                b"tick_array",
                whirlpool.as_ref(),
                &tick_start_index.to_string().as_bytes(),
            ],
            &self.program_id,
        );
        pda.to_bytes()
    }

    /// Converts internal Whirlpool index positions to standard starting bounds
    fn get_start_tick_index(&self, tick_index: i32, tick_spacing: u16) -> i32 {
        let ticks_per_array = 56 * tick_spacing as i32;
        let mut start_index = tick_index / ticks_per_array;
        if tick_index < 0 && tick_index % ticks_per_array != 0 {
            start_index -= 1;
        }
        start_index * ticks_per_array
    }
}

impl DexAdapter for WhirlpoolAdapter {
    fn protocol_name(&self) -> &'static str {
        "Whirlpool"
    }

    /// Refactored signature: Accepts a dynamic identifier counter and the current slot 
    /// to ensure global compatibility across all execution adapters.
    async fn fetch_pools(&self, client: &Client, next_id: &mut PoolId, current_slot: u64) -> Result<Vec<Pool>> {
        let response = client.get(ORCA_API_URL).send().await?.json::<OrcaApiResponse>().await?;

        let mut pools = Vec::with_capacity(response.data.len());

        for orca_pool in response.data {
            let pool_pubkey_raw = match Pubkey::from_str(&orca_pool.address) {
                Ok(pk) => pk,
                Err(e) => {
                    eprintln!("Failed to parse pool address {}: {:?}", orca_pool.address, e);
                    continue;
                }
            };

            let token_a_mint = match Self::parse_pubkey(&orca_pool.token_mint_a) {
                Ok(pk) => pk,
                Err(_) => continue,
            };
            let token_b_mint = match Self::parse_pubkey(&orca_pool.token_mint_b) {
                Ok(pk) => pk,
                Err(_) => continue,
            };
            let token_a_vault = match Self::parse_pubkey(&orca_pool.token_vault_a) {
                Ok(pk) => pk,
                Err(_) => continue,
            };
            let token_b_vault = match Self::parse_pubkey(&orca_pool.token_vault_b) {
                Ok(pk) => pk,
                Err(_) => continue,
            };

            let liquidity = u128::from_str(&orca_pool.liquidity).unwrap_or(0);
            let sqrt_price_x64 = u128::from_str(&orca_pool.sqrt_price).unwrap_or(0);

            // Filter out empty pools immediately
            if liquidity == 0 {
                continue;
            }

            // Normalize Orca fee rate (millionths) into standard basis points (bps)
            // e.g., 3000 millionths (0.3%) / 100 = 30 bps
            let fee_bps = (orca_pool.fee_rate / 100) as u16;

            // Generate localized tick arrays using deterministic on-chain logic
            let current_start_tick = self.get_start_tick_index(orca_pool.tick_current_index, orca_pool.tick_spacing);
            let active_tick_array = self.derive_tick_array_pda(&pool_pubkey_raw, current_start_tick);

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
                // Now populated with the current active tick array PDA address
                tick_array_pubkeys: vec![active_tick_array], 
            };

            pools.push(Pool {
                metadata,
                data: PoolData::Clmm(clmm_state),
                fee_bps,
                last_updated_slot: current_slot, 
            });

            // Safely advance the universal counter across adapters
            *next_id += 1;
        }

        Ok(pools)
    }
}
