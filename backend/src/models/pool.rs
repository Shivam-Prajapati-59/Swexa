use serde::{Deserialize, Deserializer, Serialize, Serializer};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Fixed-size Solana public key representation.
/// Faster than String and graph-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PubkeyBytes(pub [u8; 32]);

impl Serialize for PubkeyBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let pubkey = Pubkey::new_from_array(self.0);
        serializer.serialize_str(&pubkey.to_string())
    }
}

impl<'de> Deserialize<'de> for PubkeyBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s)
            .map(|p| PubkeyBytes(p.to_bytes()))
            .map_err(serde::de::Error::custom)
    }
}

/// Internal router pool identifier.
pub type PoolId = u32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DexProtocol {
    Raydium,
    Meteora,
    Whirlpool,
    OpenBookV2,
    Phoenix,
    Saber,
    OrcaV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PoolType {
    AMM,
    Cpmm,
    Stable,
    Clmm,
    Dlmm,
    Orderbook,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PoolStatus {
    Active,
    Disabled,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolToken {
    pub mint: PubkeyBytes,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub vault: Option<PubkeyBytes>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolMetadata {
    /// Internal router id
    pub id: PoolId,

    /// Pool account pubkey
    pub pubkey: PubkeyBytes,

    pub protocol: DexProtocol,

    pub pool_type: PoolType,

    pub status: PoolStatus,

    pub token_a: PoolToken,
    pub token_b: PoolToken,
}

/// CPMM (Raydium V4, Orca Legacy)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CpmmState {
    pub reserve_a: u128,
    pub reserve_b: u128,
}

/// Stable Pools (Saber, Meteora Stable)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StableSwapState {
    pub reserve_a: u64,
    pub reserve_b: u64,

    pub amp_factor: u64,

    pub token_a_multiplier: u64,
    pub token_b_multiplier: u64,
}

/// CLMM (Whirlpool, Raydium CLMM)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClmmState {
    pub liquidity: Option<u128>,

    pub sqrt_price_x64: Option<u128>,

    pub current_tick_index: Option<i32>,

    pub tick_spacing: u16,
}

/// DLMM (Meteora)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DlmmState {
    pub active_bin_id: Option<i32>,

    pub bin_step: u16,
}

/// Orderbook (Phoenix/OpenBook)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderbookState {
    pub market_pubkey: PubkeyBytes,

    pub base_lot_size: u64,

    pub quote_lot_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PoolData {
    Cpmm(CpmmState),
    Stable(StableSwapState),
    Clmm(ClmmState),
    Dlmm(DlmmState),
    Orderbook(OrderbookState),
}

/// Dynamic state used by routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub metadata: PoolMetadata,
    pub data: PoolData,

    /// Protocol-native fee representation.
    pub fee_rate: u32,

    pub tvl: Option<f64>,

    pub last_updated_slot: Option<u64>,
}

/// Optional metrics cache.
///
/// These should NOT be stored inside the routing graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolMetrics {
    pub tvl_usd: f64,

    pub volume_24h_usd: f64,

    pub liquidity_score: u64,
}

impl Pool {
    #[inline]
    pub fn is_active(&self) -> bool {
        matches!(self.metadata.status, PoolStatus::Active)
    }

    #[inline]
    pub fn is_stale(&self, current_slot: u64, max_slot_lag: u64) -> bool {
        self.last_updated_slot
            .map(|last_updated_slot| current_slot.saturating_sub(last_updated_slot) > max_slot_lag)
            .unwrap_or(true)
    }

    #[inline]
    pub fn get_output_token(&self, input_mint: &PubkeyBytes) -> Option<PubkeyBytes> {
        if self.metadata.token_a.mint == *input_mint {
            Some(self.metadata.token_b.mint)
        } else if self.metadata.token_b.mint == *input_mint {
            Some(self.metadata.token_a.mint)
        } else {
            None
        }
    }

    #[inline]
    pub fn is_input_token_a(&self, input_mint: &PubkeyBytes) -> bool {
        self.metadata.token_a.mint == *input_mint
    }

    #[inline]
    pub fn canonical_mint_order(&self) -> (PubkeyBytes, PubkeyBytes) {
        if self.metadata.token_a.mint < self.metadata.token_b.mint {
            (self.metadata.token_a.mint, self.metadata.token_b.mint)
        } else {
            (self.metadata.token_b.mint, self.metadata.token_a.mint)
        }
    }

    #[inline]
    pub fn pool_id(&self) -> PoolId {
        self.metadata.id
    }
}
