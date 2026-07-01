use crate::adapters::DexAdapter;
use crate::models::pool::{
    DexProtocol, DlmmState, Pool, PoolData, PoolId, PoolMetadata, PoolStatus, PoolToken, PoolType,
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
    /// UI token X amount.
    token_x_amount: serde_json::Number,
    /// UI token Y amount.
    token_y_amount: serde_json::Number,
    /// Current token Y per token X price.
    current_price: f64,
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

    fn parse_raw_amount(number: &serde_json::Number, decimals: u8) -> Result<u128> {
        let s = number.to_string();

        if s.contains('e') || s.contains('E') {
            // Parse scientific notation using integer arithmetic to avoid f64 precision loss.
            // Split on 'e'/'E' to get significand and exponent.
            let lower = s.replace('E', "e");
            let mut sci_parts = lower.split('e');
            let significand_str = sci_parts.next().unwrap_or("0");
            let exponent: i32 = sci_parts
                .next()
                .unwrap_or("0")
                .parse()
                .context("Failed to parse scientific exponent")?;

            // Parse the significand as a decimal number, then adjust total
            // shift = exponent + decimals - (fractional digits in significand).
            let mut sig_parts = significand_str.split('.');
            let sig_whole = sig_parts.next().unwrap_or("0");
            let sig_frac = sig_parts.next().unwrap_or("");
            let sig_frac_len = sig_frac.len() as i32;

            // Combine whole + frac digits into one integer string
            let base_digits = format!("{}{}", sig_whole, sig_frac);
            let base_value: u128 = base_digits
                .parse()
                .context("Failed to parse scientific significand")?;

            // Net power of 10 to multiply: exponent shifts left, sig_frac_len shifts right,
            // decimals shifts left (to convert UI amount to raw).
            let net_shift = exponent - sig_frac_len + decimals as i32;
            if net_shift < 0 {
                // Would require division (sub-atomic amounts) — truncate to zero
                return Ok(base_value / 10u128.pow((-net_shift) as u32));
            }
            return base_value
                .checked_mul(10u128.pow(net_shift as u32))
                .context("Scientific notation overflow");
        }

        let mut parts = s.split('.');
        let whole = parts.next().unwrap_or("0");
        let mut frac = parts.next().unwrap_or("").to_string();

        if frac.len() > decimals as usize {
            frac.truncate(decimals as usize);
        } else {
            frac.push_str(&"0".repeat(decimals as usize - frac.len()));
        }

        let combined = format!("{}{}", whole, frac);
        u128::from_str(&combined).context("Failed to parse raw token amount")
    }

    fn base_fee_pct_to_ppm(base_fee_pct: f64) -> Result<u32> {
        if !base_fee_pct.is_finite() || base_fee_pct < 0.0 {
            anyhow::bail!("invalid Meteora base_fee_pct");
        }

        let ppm = (base_fee_pct * 10_000.0).round();
        if ppm < 0.0 || ppm > 1_000_000.0 {
            anyhow::bail!("Meteora base_fee_pct out of range (ppm={ppm}, max=1000000)");
        }

        Ok(ppm as u32)
    }

    fn ui_price_to_raw_price(price_y_per_x: f64, decimals_x: u8, decimals_y: u8) -> Result<f64> {
        if !price_y_per_x.is_finite() || price_y_per_x <= 0.0 {
            anyhow::bail!("invalid Meteora current_price");
        }

        let decimal_scale = 10f64.powi(decimals_y as i32 - decimals_x as i32);
        let raw_price = price_y_per_x * decimal_scale;
        if !raw_price.is_finite() || raw_price <= 0.0 {
            anyhow::bail!("invalid Meteora raw price");
        }

        Ok(raw_price)
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
        let mut all_pools = Vec::new();
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

                let reserve_a =
                    match Self::parse_raw_amount(&item.token_x_amount, item.token_x.decimals as u8)
                    {
                        Ok(reserve_a) => reserve_a,
                        Err(e) => {
                            log::warn!(
                                "MeteoraAdapter: Bad token_x_amount for {}: {}",
                                item.address,
                                e
                            );
                            continue;
                        }
                    };
                let reserve_b =
                    match Self::parse_raw_amount(&item.token_y_amount, item.token_y.decimals as u8)
                    {
                        Ok(reserve_b) => reserve_b,
                        Err(e) => {
                            log::warn!(
                                "MeteoraAdapter: Bad token_y_amount for {}: {}",
                                item.address,
                                e
                            );
                            continue;
                        }
                    };

                if reserve_a == 0 || reserve_b == 0 {
                    continue;
                }

                let active_price = match Self::ui_price_to_raw_price(
                    item.current_price,
                    item.token_x.decimals as u8,
                    item.token_y.decimals as u8,
                ) {
                    Ok(active_price) => active_price,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Bad current_price for {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };

                let bin_step = match u16::try_from(item.pool_config.bin_step) {
                    Ok(0) => {
                        log::warn!(
                            "MeteoraAdapter: bin_step is 0 for {}, skipping",
                            item.address
                        );
                        continue;
                    }
                    Ok(bin_step) => bin_step,
                    Err(_) => {
                        log::warn!(
                            "MeteoraAdapter: Invalid bin_step {} for {}",
                            item.pool_config.bin_step,
                            item.address
                        );
                        continue;
                    }
                };

                // Meteora exposes base_fee_pct as percent units: 0.04 means 0.04%,
                // which is 400 ppm. dynamic_fee_pct can decay to near zero, so
                // the base fee is the stable routing input for Phase 1 quotes.
                let fee_rate = match Self::base_fee_pct_to_ppm(item.pool_config.base_fee_pct) {
                    Ok(fee_rate) => fee_rate,
                    Err(e) => {
                        log::warn!(
                            "MeteoraAdapter: Bad base_fee_pct for {}: {}",
                            item.address,
                            e
                        );
                        continue;
                    }
                };

                let metadata = PoolMetadata {
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

                all_pools.push(Pool {
                    metadata,
                    data: PoolData::Dlmm(DlmmState {
                        active_bin_id: None,
                        bin_step,
                        active_price: Some(active_price),
                        reserve_a: Some(reserve_a),
                        reserve_b: Some(reserve_b),
                    }),
                    fee_rate,
                    tvl: Some(item.tvl),
                    last_updated_slot: _current_slot,
                });

                *next_id += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_fee_pct_converts_percent_to_ppm() {
        assert_eq!(MeteoraAdapter::base_fee_pct_to_ppm(0.04).unwrap(), 400);
        assert_eq!(MeteoraAdapter::base_fee_pct_to_ppm(0.2).unwrap(), 2_000);
    }

    #[test]
    fn parse_raw_amount_converts_ui_amount_to_atomic_units() {
        let amount = serde_json::Number::from_f64(19_510.703_014_063).unwrap();
        assert_eq!(
            MeteoraAdapter::parse_raw_amount(&amount, 9).unwrap(),
            19_510_703_014_063
        );

        let amount = serde_json::Number::from_f64(1_931_560.210_851).unwrap();
        assert_eq!(
            MeteoraAdapter::parse_raw_amount(&amount, 6).unwrap(),
            1_931_560_210_851
        );
    }

    #[test]
    fn ui_price_converts_to_raw_atomic_ratio() {
        let sol_to_usdc =
            MeteoraAdapter::ui_price_to_raw_price(75.209_138_110_307_32, 9, 6).unwrap();
        assert!((sol_to_usdc - 0.075_209_138_110_307_32).abs() < 1e-15);

        let usdc_to_token_9_decimals = MeteoraAdapter::ui_price_to_raw_price(0.25, 6, 9).unwrap();
        assert_eq!(usdc_to_token_9_decimals, 250.0);
    }
}
