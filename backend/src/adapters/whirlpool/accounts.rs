use serde::Deserialize;

use crate::types::{DexProtocol, PoolEdge, PoolType, TokenMint};

// ── Orca Whirlpool API response types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OrcaToken {
    pub mint: String,
    #[serde(default)]
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Debug, Deserialize)]
pub struct OrcaWhirlpool {
    pub address: String,

    #[serde(rename = "tokenA")]
    pub token_a: OrcaToken,
    #[serde(rename = "tokenB")]
    pub token_b: OrcaToken,

    #[serde(rename = "tickSpacing")]
    pub tick_spacing: u16,
    #[serde(rename = "lpFeeRate")]
    pub lp_fee_rate: f64,
    #[serde(rename = "protocolFeeRate")]
    pub protocol_fee_rate: f64,

    pub tvl: f64,
}

#[derive(Debug, Deserialize)]
pub struct OrcaWhirlpoolListResponse {
    pub whirlpools: Vec<OrcaWhirlpool>,
}

// ── Conversion to common PoolEdge ──────────────────────────────────────────

impl From<OrcaWhirlpool> for PoolEdge {
    fn from(w: OrcaWhirlpool) -> Self {
        PoolEdge {
            address: w.address,
            dex: DexProtocol::Whirlpool,
            token_a: TokenMint {
                mint: w.token_a.mint,
                symbol: w.token_a.symbol,
                decimals: w.token_a.decimals,
            },
            token_b: TokenMint {
                mint: w.token_b.mint,
                symbol: w.token_b.symbol,
                decimals: w.token_b.decimals,
            },
            fee_rate: w.lp_fee_rate,
            tvl: w.tvl,
            // Whirlpools are always concentrated liquidity.
            pool_type: PoolType::ConcentratedLiquidity,
        }
    }
}

// ── API fetcher ────────────────────────────────────────────────────────────

/// Fetches all available pools from the Orca Whirlpool API.
pub async fn fetch_whirlpools_api() -> anyhow::Result<Vec<OrcaWhirlpool>> {
    let url = "https://api.mainnet.orca.so/v1/whirlpool/list";
    println!("Fetching pools from Orca Whirlpool API...");

    let response = reqwest::get(url).await?;
    let data: OrcaWhirlpoolListResponse = response.json().await?;

    println!(
        "Successfully fetched {} Whirlpool pools via API.",
        data.whirlpools.len()
    );

    Ok(data.whirlpools)
}
