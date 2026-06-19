use crate::adapters::DexAdapter;
use crate::models::pool::{
    ClmmState, CpmmState, DexProtocol, Pool, PoolData, PoolId, PoolMetadata, PoolStatus, PoolType,
    PubkeyBytes,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;

const RAYDIUM_API_URL: &str = "https://api-v3.raydium.io/pools/info/list-v2";
const PAGE_SIZE: u32 = 1000;
/// Minimum TVL in USD to include a pool. Filters out dead/dust pools.
const MIN_TVL_USD: f64 = 1000.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// Serde structs matching the actual Raydium V3 API response
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RaydiumApiEnvelope {
    success: bool,
    data: RaydiumPageData,
}

/// Inner page wrapper with cursor-based pagination.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RaydiumPageData {
    data: Vec<RaydiumPoolItem>,
    next_page_id: Option<String>,
}

/// A single pool entry. Covers both "Standard" (AMM) and "Concentrated" (CLMM) types.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RaydiumPoolItem {
    /// The on-chain pool account address.
    id: String,
    /// "Standard" or "Concentrated"
    #[serde(rename = "type")]
    pool_type: String,
    #[allow(dead_code)]
    program_id: String,
    #[serde(rename = "mintA")]
    mint_a: RaydiumMintInfo,
    #[serde(rename = "mintB")]
    mint_b: RaydiumMintInfo,
    /// Decimal fee rate, e.g. 0.0025 = 0.25%
    fee_rate: f64,
    tvl: f64,
    /// Captured as serde_json::Number to prevent f64 truncation on massive
    /// meme-coin supplies that exceed the 53-bit mantissa.
    mint_amount_a: serde_json::Number,
    mint_amount_b: serde_json::Number,
    /// Spot price of token A in terms of token B
    #[allow(dead_code)]
    price: f64,
    /// Only present on Concentrated pools
    #[serde(default)]
    config: Option<RaydiumClmmConfig>,
}

#[derive(Debug, Deserialize)]
struct RaydiumMintInfo {
    address: String,
    decimals: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RaydiumClmmConfig {
    tick_spacing: u16,
    #[allow(dead_code)]
    trade_fee_rate: u32,
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

pub struct RaydiumAdapter;

impl RaydiumAdapter {
    pub fn new() -> Self {
        Self
    }

    fn parse_pubkey(address: &str) -> Result<PubkeyBytes> {
        let pubkey = Pubkey::from_str(address).context("Invalid pubkey string")?;
        Ok(pubkey.to_bytes())
    }

    /// Converts the human-readable amount back to raw lamports using string
    /// manipulation. This avoids f64 floating point overflow for meme-coins with
    /// massive supplies (which frequently exceed u64::MAX and f64 bounds).
    fn parse_raw_amount(number: &serde_json::Number, decimals: u8) -> Result<u128> {
        let s = number.to_string();

        // Guard against scientific notation (e.g. "1.23e+19")
        if s.contains('e') || s.contains('E') {
            // Fall back to f64 parse → this loses precision but won't panic
            let float_val: f64 = s.parse().context("Failed to parse scientific notation")?;
            return Ok((float_val * 10f64.powi(decimals as i32)) as u128);
        }

        let mut parts = s.split('.');
        let whole = parts.next().unwrap_or("0");
        let mut frac = parts.next().unwrap_or("").to_string();

        // Shift the decimal place right by padding or truncating the fractional part
        if frac.len() > decimals as usize {
            frac.truncate(decimals as usize);
        } else {
            frac.push_str(&"0".repeat(decimals as usize - frac.len()));
        }

        let combined = format!("{}{}", whole, frac);
        u128::from_str(&combined).context("Failed to parse raw token amount")
    }

    /// Fetches a single page from the Raydium API with retry + timeout.
    async fn fetch_page(client: &Client, cursor: &Option<String>) -> Result<RaydiumApiEnvelope> {
        let mut last_err = None;

        for attempt in 0..MAX_RETRIES {
            let mut request = client
                .get(RAYDIUM_API_URL)
                .timeout(REQUEST_TIMEOUT)
                .query(&[("size", &PAGE_SIZE.to_string())]);

            if let Some(page_id) = cursor {
                request = request.query(&[("nextPageId", page_id)]);
            }

            match request.send().await {
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => return resp.json().await.context("Failed to parse Raydium JSON"),
                    Err(e) => {
                        log::warn!(
                            "RaydiumAdapter: HTTP {} on attempt {}/{}",
                            e.status().map_or("unknown".into(), |s| s.to_string()),
                            attempt + 1,
                            MAX_RETRIES,
                        );
                        last_err = Some(e.into());
                    }
                },
                Err(e) => {
                    log::warn!(
                        "RaydiumAdapter: Request failed on attempt {}/{}: {}",
                        attempt + 1,
                        MAX_RETRIES,
                        e,
                    );
                    last_err = Some(e.into());
                }
            }

            // Exponential backoff: 500ms, 1s, 2s
            tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Raydium fetch failed after retries")))
    }
}

impl DexAdapter for RaydiumAdapter {
    fn protocol_name(&self) -> &'static str {
        "Raydium"
    }

    async fn fetch_pools(
        &self,
        client: &Client,
        next_id: &mut PoolId,
        current_slot: u64,
    ) -> Result<Vec<Pool>> {
        let mut all_pools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut page_num = 0u32;

        loop {
            let envelope = Self::fetch_page(client, &cursor).await?;

            if !envelope.success {
                anyhow::bail!("Raydium API returned success=false");
            }

            let page = envelope.data;
            page_num += 1;
            log::debug!(
                "RaydiumAdapter: Processing page {}, {} pools",
                page_num,
                page.data.len()
            );

            for item in page.data {
                // Skip dust / dead pools
                if item.tvl < MIN_TVL_USD {
                    continue;
                }

                let pubkey = match Self::parse_pubkey(&item.id) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!("RaydiumAdapter: Invalid pool address {}: {}", item.id, e);
                        continue;
                    }
                };
                let token_a_mint = match Self::parse_pubkey(&item.mint_a.address) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!("RaydiumAdapter: Invalid mintA for {}: {}", item.id, e);
                        continue;
                    }
                };
                let token_b_mint = match Self::parse_pubkey(&item.mint_b.address) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!("RaydiumAdapter: Invalid mintB for {}: {}", item.id, e);
                        continue;
                    }
                };

                // Raydium CPMM vaults are standard token accounts created at launch.
                // They are NOT PDAs. You MUST fetch the pool state via RPC to resolve these.
                // We leave them empty here to be hydrated by the execution engine later.
                let token_a_vault = [0u8; 32];
                let token_b_vault = [0u8; 32];

                // FIX #6: Use .round() to prevent truncation (2499.999 → 2500, not 2499)
                let fee_rate = (item.fee_rate * 1_000_000.0).round() as u32;

                let (pool_type, pool_data) = match item.pool_type.as_str() {
                    // -------------------------------------------------------
                    // Standard AMM (Constant Product)
                    // -------------------------------------------------------
                    "Standard" => {
                        let reserve_a =
                            match Self::parse_raw_amount(&item.mint_amount_a, item.mint_a.decimals)
                            {
                                Ok(r) => r,
                                Err(e) => {
                                    log::warn!(
                                        "RaydiumAdapter: Bad reserve_a for {}: {}",
                                        item.id,
                                        e
                                    );
                                    continue;
                                }
                            };
                        let reserve_b =
                            match Self::parse_raw_amount(&item.mint_amount_b, item.mint_b.decimals)
                            {
                                Ok(r) => r,
                                Err(e) => {
                                    log::warn!(
                                        "RaydiumAdapter: Bad reserve_b for {}: {}",
                                        item.id,
                                        e
                                    );
                                    continue;
                                }
                            };

                        if reserve_a == 0 || reserve_b == 0 {
                            continue;
                        }

                        (
                            PoolType::Cpmm,
                            PoolData::Cpmm(CpmmState {
                                reserve_a,
                                reserve_b,
                            }),
                        )
                    }

                    // -------------------------------------------------------
                    // Concentrated Liquidity (CLMM)
                    // FIX #2: We do NOT approximate liquidity or sqrt_price here.
                    // The REST API cannot provide safe on-chain parameters for CLMMs.
                    // This pool is discovered for graph building, but its state MUST
                    // be hydrated via RPC `getMultipleAccounts` before quoting.
                    // -------------------------------------------------------
                    "Concentrated" => {
                        let config = match &item.config {
                            Some(c) => c,
                            None => {
                                log::warn!(
                                    "RaydiumAdapter: CLMM pool {} missing config block, skipping",
                                    item.id
                                );
                                continue;
                            }
                        };

                        (
                            PoolType::Clmm,
                            PoolData::Clmm(ClmmState {
                                liquidity: 0,
                                sqrt_price_x64: 0,
                                current_tick_index: 0,
                                tick_spacing: config.tick_spacing,
                                // Todo: Raydium CLMM tick arrays fetched on-demand at quote time
                                tick_array_pubkeys: vec![],
                            }),
                        )
                    }

                    other => {
                        log::warn!(
                            "RaydiumAdapter: Unknown pool type '{}' for {}, skipping",
                            other,
                            item.id
                        );
                        continue;
                    }
                };

                let metadata = PoolMetadata {
                    id: *next_id,
                    pubkey,
                    protocol: DexProtocol::Raydium,
                    pool_type,
                    status: PoolStatus::Active,
                    token_a_mint,
                    token_b_mint,
                    token_a_decimals: item.mint_a.decimals,
                    token_b_decimals: item.mint_b.decimals,
                    token_a_vault,
                    token_b_vault,
                };

                all_pools.push(Pool {
                    metadata,
                    data: pool_data,
                    fee_rate,
                    last_updated_slot: current_slot,
                });

                *next_id += 1;
            }

            // Advance to next page or break
            cursor = page.next_page_id;
            log::debug!("RaydiumAdapter: next_page_id={:?}", cursor);
            if cursor.is_none() {
                break;
            }
        }

        log::info!(
            "RaydiumAdapter: Fetched {} pools across {} pages",
            all_pools.len(),
            page_num
        );

        Ok(all_pools)
    }
}
