use serde::Deserialize;

use crate::types::{DexProtocol, PoolEdge, PoolType, TokenMint};

// ── Raydium V3 API response types ──────────────────────────────────────────

/// Token information as returned by the Raydium API (`mintA` / `mintB`).
#[derive(Debug, Deserialize)]
pub struct RaydiumMintInfo {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
}

/// Pool-level config object (nested inside each pool entry).
#[derive(Debug, Deserialize)]
pub struct RaydiumPoolConfig {
    #[serde(rename = "tickSpacing")]
    pub tick_spacing: Option<u16>,
}

/// A single pool entry from the Raydium V3 `/pools/info/list` endpoint.
#[derive(Debug, Deserialize)]
pub struct RaydiumPoolInfo {
    /// On-chain pool address.
    pub id: String,

    /// Pool mechanism type — "Concentrated", "Standard", etc.
    #[serde(rename = "type")]
    pub pool_type: String,

    #[serde(rename = "mintA")]
    pub mint_a: RaydiumMintInfo,

    #[serde(rename = "mintB")]
    pub mint_b: RaydiumMintInfo,

    /// Trading fee as a fraction (e.g. 0.0025 = 0.25 %).
    #[serde(rename = "feeRate")]
    pub fee_rate: f64,

    /// Total Value Locked in USD.
    pub tvl: f64,

    /// Optional nested config (present on CLMM pools).
    pub config: Option<RaydiumPoolConfig>,
}

/// Inner `data` wrapper: `{ count, data: [...] }`.
#[derive(Debug, Deserialize)]
pub struct RaydiumDataInner {
    pub count: u64,
    pub data: Vec<RaydiumPoolInfo>,
}

/// Top-level Raydium V3 API response envelope.
#[derive(Debug, Deserialize)]
pub struct RaydiumApiResponse {
    pub success: bool,
    pub data: RaydiumDataInner,
}

// ── Conversion to common PoolEdge ──────────────────────────────────────────

impl From<RaydiumPoolInfo> for PoolEdge {
    fn from(p: RaydiumPoolInfo) -> Self {
        let pool_type = match p.pool_type.as_str() {
            "Concentrated" => PoolType::ConcentratedLiquidity,
            _ => PoolType::Amm,
        };

        PoolEdge {
            address: p.id,
            dex: DexProtocol::Raydium,
            token_a: TokenMint {
                mint: p.mint_a.address,
                symbol: p.mint_a.symbol,
                decimals: p.mint_a.decimals,
            },
            token_b: TokenMint {
                mint: p.mint_b.address,
                symbol: p.mint_b.symbol,
                decimals: p.mint_b.decimals,
            },
            fee_rate: p.fee_rate,
            tvl: p.tvl,
            pool_type,
        }
    }
}

// ── API fetcher ────────────────────────────────────────────────────────────

/// Fetches all available pools from the Raydium V3 API (top 1000 by TVL).
pub async fn fetch_raydium_pools() -> anyhow::Result<Vec<RaydiumPoolInfo>> {
    let url = "https://api-v3.raydium.io/pools/info/list?poolType=all&poolSortField=liquidity&sortType=desc&pageSize=1000&page=1";
    println!("Fetching pools from Raydium API...");

    let response = reqwest::get(url).await?;
    let api_resp: RaydiumApiResponse = response.json().await?;

    if !api_resp.success {
        anyhow::bail!("Raydium API returned success=false");
    }

    println!(
        "Successfully fetched {} Raydium pools via API.",
        api_resp.data.data.len()
    );

    Ok(api_resp.data.data)
}
