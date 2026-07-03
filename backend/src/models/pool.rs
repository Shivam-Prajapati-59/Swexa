use crate::models::sim_error::SimulationError;
use crate::models::swap_math::{
    DlmmQuoteParams, SwapResult, simulate_clmm_tick_traversal, simulate_clmm_virtual_reserves,
    simulate_cpmm, simulate_dlmm_bin_traversal, simulate_dlmm_spot, simulate_stableswap,
};
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
    pub reserve_a: u128,
    pub reserve_b: u128,

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

    pub reserve_a: Option<u128>,
    pub reserve_b: Option<u128>,

    pub initialized_ticks: Vec<ClmmTick>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClmmTick {
    pub index: i32,
    pub liquidity_net: i128,
}

/// DLMM (Meteora)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DlmmState {
    pub active_bin_id: Option<i32>,

    pub bin_step: u16,

    pub active_price: Option<f64>,

    pub reserve_a: Option<u128>,
    pub reserve_b: Option<u128>,

    pub bins: Vec<DlmmBin>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DlmmBin {
    pub id: i32,
    pub amount_x: u128,
    pub amount_y: u128,
}

/// Orderbook (Phoenix/OpenBook)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderbookState {
    pub market_pubkey: PubkeyBytes,

    pub base_lot_size: u64,

    pub quote_lot_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

    /// Simulates an exact-input swap through this pool using pool-type-specific math.
    ///
    /// CPMM quotes are fixed-point exact. StableSwap uses a bounded f64 Newton
    /// solver. CLMM/DLMM hydrate nearby on-chain tick/bin liquidity when it is
    /// available and otherwise fall back to guarded spot approximations.
    pub fn simulate_swap(
        &self,
        input_mint: &PubkeyBytes,
        amount_in: u128,
    ) -> Result<SwapResult, SimulationError> {
        if self.metadata.token_a.mint != *input_mint && self.metadata.token_b.mint != *input_mint {
            return Err(SimulationError::MismatchedMint);
        }
        if amount_in == 0 {
            return Err(SimulationError::ZeroInput);
        }

        let is_a_in = self.is_input_token_a(input_mint);

        match &self.data {
            PoolData::Cpmm(state) => {
                let (reserve_in, reserve_out) = if is_a_in {
                    (state.reserve_a, state.reserve_b)
                } else {
                    (state.reserve_b, state.reserve_a)
                };
                simulate_cpmm(amount_in, reserve_in, reserve_out, self.fee_rate, false)
            }
            PoolData::Stable(state) => {
                let (res_in, res_out, mult_in, mult_out) = if is_a_in {
                    (
                        state.reserve_a,
                        state.reserve_b,
                        state.token_a_multiplier,
                        state.token_b_multiplier,
                    )
                } else {
                    (
                        state.reserve_b,
                        state.reserve_a,
                        state.token_b_multiplier,
                        state.token_a_multiplier,
                    )
                };
                simulate_stableswap(
                    amount_in,
                    res_in,
                    res_out,
                    mult_in,
                    mult_out,
                    state.amp_factor,
                    self.fee_rate,
                )
            }
            PoolData::Clmm(state) => {
                let liquidity = state
                    .liquidity
                    .ok_or(SimulationError::InsufficientLiquidity)?;
                let sqrt_price_x64 = state
                    .sqrt_price_x64
                    .ok_or(SimulationError::InsufficientLiquidity)?;
                let reserve_out = if is_a_in {
                    state.reserve_b
                } else {
                    state.reserve_a
                };
                let result = if state.initialized_ticks.is_empty() {
                    simulate_clmm_virtual_reserves(
                        amount_in,
                        liquidity,
                        sqrt_price_x64,
                        is_a_in,
                        self.fee_rate,
                    )?
                } else {
                    let current_tick_index = state
                        .current_tick_index
                        .ok_or(SimulationError::InsufficientLiquidity)?;
                    simulate_clmm_tick_traversal(
                        amount_in,
                        liquidity,
                        sqrt_price_x64,
                        current_tick_index,
                        &state.initialized_ticks,
                        is_a_in,
                        self.fee_rate,
                    )?
                };

                if let Some(reserve_out) = reserve_out
                    && result.amount_out > reserve_out
                {
                    return Err(SimulationError::InsufficientLiquidity);
                }
                Ok(result)
            }
            PoolData::Dlmm(state) => {
                if state.active_bin_id.is_none() && state.active_price.is_none() {
                    return Err(SimulationError::InsufficientLiquidity);
                }
                let (reserve_in, reserve_out) = if is_a_in {
                    (state.reserve_a, state.reserve_b)
                } else {
                    (state.reserve_b, state.reserve_a)
                };
                let params = DlmmQuoteParams {
                    bin_step: state.bin_step,
                    active_bin_id: state.active_bin_id,
                    active_price: state.active_price,
                    bins: &state.bins,
                    reserve_in,
                    reserve_out,
                    a_to_b: is_a_in,
                    fee_rate_ppm: self.fee_rate,
                };
                let result = if state.bins.is_empty() {
                    simulate_dlmm_spot(amount_in, &params)?
                } else {
                    simulate_dlmm_bin_traversal(amount_in, &params)?
                };
                Ok(result)
            }
            PoolData::Orderbook(_) => Err(SimulationError::UnsupportedPoolType),
        }
    }

    /// Derives the current raw spot price of the output token in terms of the input token.
    /// Price = amount of raw output token received per 1 raw unit of input token (before fees).
    /// Returns `None` if `input_mint` does not match either token in this pool.
    pub fn spot_price(&self, input_mint: &PubkeyBytes) -> Option<f64> {
        // Reject mints that don't belong to this pool
        if self.metadata.token_a.mint != *input_mint && self.metadata.token_b.mint != *input_mint {
            return None;
        }

        let is_a_in = self.is_input_token_a(input_mint);

        match &self.data {
            PoolData::Cpmm(state) => {
                let (res_in, res_out) = if is_a_in {
                    (state.reserve_a as f64, state.reserve_b as f64)
                } else {
                    (state.reserve_b as f64, state.reserve_a as f64)
                };
                if res_in > 0.0 {
                    Some(res_out / res_in)
                } else {
                    None
                }
            }
            PoolData::Clmm(state) => {
                // CLMM sqrt_price_x64 is (sqrt(P) * 2^64). Price P = Y/X (Token B per Token A)
                if let Some(sqrt_price) = state.sqrt_price_x64 {
                    let price = (sqrt_price as f64 / (1u128 << 64) as f64).powi(2);
                    if is_a_in {
                        Some(price)
                    } else if price > 0.0 {
                        Some(1.0 / price)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            PoolData::Dlmm(state) => {
                if let Some(price) = state.active_price {
                    if !price.is_finite() || price <= 0.0 {
                        return None;
                    }
                    if is_a_in {
                        Some(price)
                    } else {
                        Some(1.0 / price)
                    }
                } else if let Some(bin_id) = state.active_bin_id {
                    // DLMM price = (1 + bin_step/10000) ^ active_bin_id
                    let base = 1.0 + (state.bin_step as f64 / 10_000.0);
                    let price = base.powf(bin_id as f64);
                    if is_a_in {
                        Some(price)
                    } else if price > 0.0 {
                        Some(1.0 / price)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            PoolData::Stable(state) => {
                // Proper stableswap spot price using the invariant:
                // A * 4 * (X + Y) + D = A * 4 * D + D^3 / (4 * X * Y)
                // where X, Y are normalized reserves (raw * multiplier).
                let (res_in, res_out, mult_in, mult_out) = if is_a_in {
                    (
                        state.reserve_a,
                        state.reserve_b,
                        state.token_a_multiplier,
                        state.token_b_multiplier,
                    )
                } else {
                    (
                        state.reserve_b,
                        state.reserve_a,
                        state.token_b_multiplier,
                        state.token_a_multiplier,
                    )
                };

                let x = (res_in as f64) * (mult_in as f64);
                let y = (res_out as f64) * (mult_out as f64);

                if x <= 0.0 || y <= 0.0 {
                    return None;
                }

                let a = state.amp_factor as f64;

                // Solve for D (invariant) via Newton's method
                // D_next = (16*A*x*y*(x+y) + 2*D^3) / (16*A*x*y + 3*D^2)
                let mut d = x + y;
                for _ in 0..64 {
                    let d2 = d * d;
                    let d3 = d2 * d;
                    let xy = x * y;
                    let num = 16.0 * a * xy * (x + y) + 2.0 * d3;
                    let den = 16.0 * a * xy + 3.0 * d2;
                    if den <= 0.0 || den.is_nan() {
                        break;
                    }
                    let d_next = num / den;
                    if (d_next - d).abs() <= 1.0 {
                        d = d_next;
                        break;
                    }
                    d = d_next;
                }

                if d <= 0.0 {
                    return None;
                }

                // Spot price = -dY/dX = (4*A + D^3/(4*X^2*Y)) / (4*A + D^3/(4*X*Y^2))
                // Both numerator and denominator are positive, so spot is positive.
                let d3 = d * d * d;
                let four_a = 4.0 * a;
                let xy_term = d3 / (4.0 * x * y);
                let spot = (four_a + xy_term / x) / (four_a + xy_term / y) * (mult_in as f64)
                    / (mult_out as f64);

                if spot.is_finite() && spot > 0.0 {
                    Some(spot)
                } else {
                    None
                }
            }
            PoolData::Orderbook(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint(byte: u8) -> PubkeyBytes {
        PubkeyBytes([byte; 32])
    }

    fn clmm_pool(reserve_a: Option<u128>, reserve_b: Option<u128>) -> Pool {
        let token_a = mint(1);
        let token_b = mint(2);
        Pool {
            metadata: PoolMetadata {
                id: 1,
                pubkey: mint(9),
                protocol: DexProtocol::Whirlpool,
                pool_type: PoolType::Clmm,
                status: PoolStatus::Active,
                token_a: PoolToken {
                    mint: token_a,
                    name: "A".to_string(),
                    symbol: "A".to_string(),
                    decimals: 6,
                    vault: Some(mint(3)),
                },
                token_b: PoolToken {
                    mint: token_b,
                    name: "B".to_string(),
                    symbol: "B".to_string(),
                    decimals: 6,
                    vault: Some(mint(4)),
                },
            },
            data: PoolData::Clmm(ClmmState {
                liquidity: Some(1_000_000),
                sqrt_price_x64: Some(1u128 << 64),
                current_tick_index: Some(0),
                tick_spacing: 1,
                reserve_a,
                reserve_b,
                initialized_ticks: Vec::new(),
            }),
            fee_rate: 0,
            tvl: Some(1_000.0),
            last_updated_slot: Some(1),
        }
    }

    #[test]
    fn clmm_quote_respects_hydrated_output_reserve() {
        let pool = clmm_pool(Some(1_000_000), Some(10));

        assert_eq!(
            pool.simulate_swap(&mint(1), 1_000).unwrap_err(),
            SimulationError::InsufficientLiquidity
        );
    }

    #[test]
    fn clmm_quote_allows_missing_reserve_caps() {
        let pool = clmm_pool(None, None);

        let result = pool.simulate_swap(&mint(1), 1_000).unwrap();
        assert!(result.amount_out > 0);
        assert!(result.is_approximate);
    }

    #[test]
    fn dlmm_sol_usdc_raw_price_does_not_overquote_by_decimals() {
        let sol = mint(1);
        let usdc = mint(2);
        let pool = Pool {
            metadata: PoolMetadata {
                id: 2,
                pubkey: mint(8),
                protocol: DexProtocol::Meteora,
                pool_type: PoolType::Dlmm,
                status: PoolStatus::Active,
                token_a: PoolToken {
                    mint: sol,
                    name: "Wrapped SOL".to_string(),
                    symbol: "SOL".to_string(),
                    decimals: 9,
                    vault: Some(mint(3)),
                },
                token_b: PoolToken {
                    mint: usdc,
                    name: "USD Coin".to_string(),
                    symbol: "USDC".to_string(),
                    decimals: 6,
                    vault: Some(mint(4)),
                },
            },
            data: PoolData::Dlmm(DlmmState {
                active_bin_id: None,
                bin_step: 4,
                active_price: Some(0.075),
                reserve_a: Some(2_000_000_000),
                reserve_b: Some(100_000_000),
                bins: Vec::new(),
            }),
            fee_rate: 0,
            tvl: Some(1_000_000.0),
            last_updated_slot: Some(1),
        };

        let result = pool.simulate_swap(&sol, 1_000_000_000).unwrap();

        assert_eq!(result.amount_out, 75_000_000);
    }
}
