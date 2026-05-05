use crate::types::{DexProtocol, PoolEdge, PoolType, TokenMint};
use anyhow::Result;
use serde::Deserialize;

const METEORA_POOLS_API: &str = "https://dlmm.datapi.meteora.ag/pools";
const METEORA_PAGE_SIZE: u64 = 1000;
const MAX_PAGES_TO_FETCH: u64 = 5;

#[derive(Debug, Deserialize)]
pub struct MeteoraResponse {
    #[serde(rename = "total")]
    pub _total: u64,
    pub pages: u64,
    #[serde(rename = "current_page")]
    pub _current_page: u64,
    pub data: Vec<MeteoraPool>,
}

#[derive(Debug, Deserialize)]
pub struct MeteoraPool {
    pub address: String,
    pub token_x: MeteoraToken,
    pub token_y: MeteoraToken,
    pub pool_config: MeteoraPoolConfig,
    pub tvl: f64,
}

#[derive(Debug, Deserialize)]
pub struct MeteoraToken {
    pub address: String,
    pub symbol: Option<String>,
    pub decimals: u8,
}

#[derive(Debug, Deserialize)]
pub struct MeteoraPoolConfig {
    pub base_fee_pct: f64,
}

impl From<MeteoraPool> for PoolEdge {
    fn from(pool: MeteoraPool) -> Self {
        Self {
            address: pool.address,
            dex: DexProtocol::Meteora,
            pool_type: PoolType::Dlmm, // Meteora is DLMM
            tvl: pool.tvl,
            fee_rate: pool.pool_config.base_fee_pct / 100.0, // convert percentage to fraction
            token_a: TokenMint {
                mint: pool.token_x.address,
                symbol: pool.token_x.symbol.unwrap_or_else(|| "Unknown".to_string()),
                decimals: pool.token_x.decimals,
            },
            token_b: TokenMint {
                mint: pool.token_y.address,
                symbol: pool.token_y.symbol.unwrap_or_else(|| "Unknown".to_string()),
                decimals: pool.token_y.decimals,
            },
        }
    }
}

pub async fn fetch_meteora_pools(min_tvl: f64) -> Result<Vec<MeteoraPool>> {
    let client = reqwest::Client::new();
    let mut all_pools = Vec::new();

    // Fetch pages sequentially to respect rate limits (30 requests per second limit)
    let mut page = 1;
    loop {
        let url = format!(
            "{}?page_size={}&page={}&sort_by=tvl:desc&filter_by=tvl%3E%3D{}",
            METEORA_POOLS_API, METEORA_PAGE_SIZE, page, min_tvl
        );

        match client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<MeteoraResponse>().await {
                        Ok(mut json_response) => {
                            if json_response.data.is_empty() {
                                break;
                            }

                            // Find the max TVL in this page to check if we should keep fetching
                            let max_tvl_in_page =
                                json_response.data.iter().map(|p| p.tvl).fold(0.0, f64::max);

                            all_pools.append(&mut json_response.data);

                            // Break early if we've reached the last page or if TVL drops below threshold
                            if page >= json_response.pages
                                || max_tvl_in_page < min_tvl
                                || page >= MAX_PAGES_TO_FETCH
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("[Meteora] Failed to parse JSON on page {}: {}", page, e);
                            break;
                        }
                    }
                } else {
                    eprintln!("[Meteora] API returned error status: {}", resp.status());
                    break;
                }
            }
            Err(e) => {
                eprintln!("[Meteora] Network error on page {}: {}", page, e);
                break;
            }
        }
        page += 1;
    }

    Ok(all_pools)
}
