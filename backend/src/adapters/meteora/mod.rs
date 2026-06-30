use crate::adapters::DexAdapter;
use crate::models::pool::{
    DexProtocol, Pool, PoolId, PoolMetadata, PoolStatus, PoolToken, PoolType,
    PubkeyBytes,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;

const METEORA_API_URL: &str = "https://dlmm.datapi.meteora.ag/pools";
const PAGE_SIZE: u32 = 1000;
const MIN_TVL_USD: f64 = 1000.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// Serde structs matching the Meteora DLMM API response
// ---------------------------------------------------------------------------

/// Envelope: `{ total, pages, current_page, page_size, data: [...] }`
#[derive(Debug, Deserialize)]
struct MeteoraApiResponse {
    #[allow(dead_code)]
    total: u64,
    pages: u64,
    #[allow(dead_code)]
    current_page: u64,
    #[allow(dead_code)]
    page_size: u64,
    data: Vec<MeteoraPoolItem>,
}

/// A single DLMM pool entry.
/// Field names use snake_case matching the Meteora API natively.
#[derive(Debug, Deserialize)]
struct MeteoraPoolItem {
    /// On-chain pool account address.
    address: String,
    /// Token X metadata.
    token_x: MeteoraTokenInfo,
    /// Token Y metadata.
    token_y: MeteoraTokenInfo,
    /// Reserve X vault address.
    reserve_x: String,
    /// Reserve Y vault address.
    reserve_y: String,
    /// Dynamic fee rate (base + variable). Can decay to near-zero (e.g. 5.1e-6)
    /// during low-volatility periods — NOT reliable for routing.
    #[allow(dead_code)]
    dynamic_fee_pct: f64,
    /// Total Value Locked in USD.
    tvl: f64,
    /// Pool configuration (bin_step, base_fee, etc.)
    pool_config: MeteoraPoolConfig,
    /// Whether the pair is blacklisted.
    is_blacklisted: bool,
}

#[derive(Debug, Deserialize)]
struct MeteoraTokenInfo {
    address: String,
    decimals: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct MeteoraPoolConfig {
    /// Bin step of the pool (price granularity).
    bin_step: u32,
    /// Base fee rate as a percentage, e.g. 0.04 = 0.04%
    /// This is the stable minimum fee — used for routing instead of dynamic_fee_pct.
    base_fee_pct: f64,
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct MeteoraAdapter;

impl MeteoraAdapter {
    fn parse_pubkey(address: &str) -> Result<PubkeyBytes> {
        let pubkey = Pubkey::from_str(address).context("Invalid pubkey string")?;
        Ok(PubkeyBytes(pubkey.to_bytes()))
    }

    /// Fetches a single page from the Meteora API with retry + timeout.
    async fn fetch_page(client: &Client, page: u32) -> Result<MeteoraApiResponse> {
        let mut last_err = None;

        for attempt in 0..MAX_RETRIES {
            let result = client
                .get(METEORA_API_URL)
                .timeout(REQUEST_TIMEOUT)
                .query(&[
                    ("page", &page.to_string()),
                    ("page_size", &PAGE_SIZE.to_string()),
                    ("filter_by", &"is_blacklisted=false".to_string()),
                    ("sort_by", &"tvl:desc".to_string()),
                ])
                .send()
                .await;

            match result {
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => return resp.json().await.context("Failed to parse Meteora JSON"),
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: HTTP {} on attempt {}/{}",
                            e.status().map_or("unknown".into(), |s| s.to_string()),
                            attempt + 1,
                            MAX_RETRIES,
                        );
                        last_err = Some(e.into());
                    }
                },
                Err(e) => {
                    log::warn!(
                        "MeteoraAdapter: Request failed on attempt {}/{}: {}",
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

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Meteora fetch failed after retries")))
    }
}

impl DexAdapter for MeteoraAdapter {
    fn protocol_name(&self) -> &'static str {
        "Meteora"
    }

    async fn fetch_pools(
        &self,
        client: &Client,
        next_id: &mut PoolId,
        _current_slot: Option<u64>,
    ) -> Result<Vec<Pool>> {
        let all_pools = Vec::new();
        let mut current_page = 1u32;

        loop {
            let response = Self::fetch_page(client, current_page).await?;
            let total_pages = response.pages;

            log::debug!(
                "MeteoraAdapter: Processing page {}/{}, {} pools",
                current_page,
                total_pages,
                response.data.len()
            );

            // If sorted by TVL descending and we get a page where all pools
            // are below threshold, we can stop early.
            let mut all_below_threshold = true;

            for item in response.data {
                // Skip blacklisted (should already be filtered by query param, but double-check)
                if item.is_blacklisted {
                    continue;
                }

                // Skip dust / dead pools
                if item.tvl < MIN_TVL_USD {
                    continue;
                }

                all_below_threshold = false;

                let pubkey = match Self::parse_pubkey(&item.address) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Invalid pool address {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };
                let token_a_mint = match Self::parse_pubkey(&item.token_x.address) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Invalid token_x for {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };
                let token_b_mint = match Self::parse_pubkey(&item.token_y.address) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Invalid token_y for {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };

                // Meteora exposes reserve vault addresses directly
                let token_a_vault = match Self::parse_pubkey(&item.reserve_x) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Invalid reserve_x for {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };
                let token_b_vault = match Self::parse_pubkey(&item.reserve_y) {
                    Ok(pk) => pk,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Invalid reserve_y for {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };

                // Use base_fee_pct from pool_config as the stable routing fee.
                // dynamic_fee_pct can decay to ~0 (e.g. 5.1e-6) during calm markets.
                let _fee_rate = (item.pool_config.base_fee_pct * 10_000.0).round() as u32;

                let _metadata = PoolMetadata {
                    id: *next_id,
                    pubkey,
                    protocol: DexProtocol::Meteora,
                    pool_type: PoolType::Dlmm,
                    status: PoolStatus::Active,
                    token_a: PoolToken {
                        mint: token_a_mint,
                        name: item.token_x.name.clone(),
                        symbol: item.token_x.symbol.clone(),
                        decimals: item.token_x.decimals as u8,
                        vault: Some(token_a_vault),
                    },
                    token_b: PoolToken {
                        mint: token_b_mint,
                        name: item.token_y.name.clone(),
                        symbol: item.token_y.symbol.clone(),
                        decimals: item.token_y.decimals as u8,
                        vault: Some(token_b_vault),
                    },
                };

                // DLMM state — active_bin_id is not available from this endpoint.
                // Must be hydrated via RPC before quoting. Until hydration exists,
                // skip these pools since they cannot be priced by the optimizer.
                log::debug!(
                    "MeteoraAdapter: Skipping pool {} (active_bin_id requires RPC hydration)",
                    item.address
                );
                continue;
            }

            // Early termination: since we sort by TVL desc, once an entire page
            // is below threshold there's no point fetching more pages.
            if all_below_threshold {
                log::debug!(
                    "MeteoraAdapter: All pools on page {} below TVL threshold, stopping early",
                    current_page
                );
                break;
            }

            // Check if we've processed all pages
            if current_page as u64 >= total_pages {
                break;
            }

            current_page += 1;
        }

        log::info!(
            "MeteoraAdapter: Fetched {} pools across {} pages",
            all_pools.len(),
            current_page
        );

        Ok(all_pools)
    }
}
