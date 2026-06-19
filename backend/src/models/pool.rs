use serde::{Deserialize, Serialize};

/// Fixed-size Solana public key representation.
/// Faster than String and graph-friendly.
pub type PubkeyBytes = [u8; 32];

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
pub struct PoolMetadata {
    /// Internal router id
    pub id: PoolId,

    /// Pool account pubkey
    pub pubkey: PubkeyBytes,

    pub protocol: DexProtocol,

    pub pool_type: PoolType,

    pub status: PoolStatus,

    pub token_a_mint: PubkeyBytes,
    pub token_b_mint: PubkeyBytes,

    pub token_a_decimals: u8,
    pub token_b_decimals: u8,

    pub token_a_vault: PubkeyBytes,
    pub token_b_vault: PubkeyBytes,
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
    pub liquidity: u128,

    pub sqrt_price_x64: u128,

    pub current_tick_index: i32,

    pub tick_spacing: u16,

    /// Tick array accounts
    pub tick_array_pubkeys: Vec<PubkeyBytes>,
}

/// DLMM (Meteora)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DlmmState {
    pub active_bin_id: i32,

    pub bin_step: u16,

    /// Bin array accounts
    pub bin_array_pubkeys: Vec<PubkeyBytes>,
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

    pub last_updated_slot: u64,
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
        current_slot.saturating_sub(self.last_updated_slot) > max_slot_lag
    }

    #[inline]
    pub fn get_output_mint(&self, input_mint: &PubkeyBytes) -> Option<PubkeyBytes> {
        if self.metadata.token_a_mint == *input_mint {
            Some(self.metadata.token_b_mint)
        } else if self.metadata.token_b_mint == *input_mint {
            Some(self.metadata.token_a_mint)
        } else {
            None
        }
    }

    #[inline]
    pub fn is_input_token_a(&self, input_mint: &PubkeyBytes) -> bool {
        self.metadata.token_a_mint == *input_mint
    }

    #[inline]
    pub fn canonical_pair_key(&self) -> (PubkeyBytes, PubkeyBytes) {
        if self.metadata.token_a_mint < self.metadata.token_b_mint {
            (self.metadata.token_a_mint, self.metadata.token_b_mint)
        } else {
            (self.metadata.token_b_mint, self.metadata.token_a_mint)
        }
    }

    #[inline]
    pub fn pool_id(&self) -> PoolId {
        self.metadata.id
    }
}
