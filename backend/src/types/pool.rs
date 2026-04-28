use serde::{Deserialize, Serialize};
use std::fmt;

// ── Protocol Enum ──────────────────────────────────────────────────────────

/// Identifies which DEX a pool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DexProtocol {
    Whirlpool,
    Raydium,
}

impl fmt::Display for DexProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DexProtocol::Whirlpool => write!(f, "Whirlpool"),
            DexProtocol::Raydium => write!(f, "Raydium"),
        }
    }
}

// ── Pool Type Enum ─────────────────────────────────────────────────────────

/// Broad classification of the AMM mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolType {
    /// Standard constant-product (x * y = k) AMM.
    Amm,
    /// Concentrated-liquidity market maker (tick-based).
    ConcentratedLiquidity,
}

impl fmt::Display for PoolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoolType::Amm => write!(f, "AMM"),
            PoolType::ConcentratedLiquidity => write!(f, "CLMM"),
        }
    }
}

// ── Token Mint Info ────────────────────────────────────────────────────────

/// Minimal token information needed for routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMint {
    /// SPL token mint address (base58).
    pub mint: String,
    /// Human-readable symbol (e.g. "SOL", "USDC"). May be empty if the API
    /// does not provide one.
    pub symbol: String,
    /// Number of decimals for this token.
    pub decimals: u8,
}

// ── Pool Edge ──────────────────────────────────────────────────────────────

/// A protocol-agnostic pool representation used by the routing engine.
///
/// Every adapter converts its DEX-specific response into this common type so
/// the router only ever works with a homogeneous `Vec<PoolEdge>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolEdge {
    /// On-chain pool / market address.
    pub address: String,
    /// Which DEX owns this pool.
    pub dex: DexProtocol,
    /// The first token in the pair.
    pub token_a: TokenMint,
    /// The second token in the pair.
    pub token_b: TokenMint,
    /// Trading fee expressed as a fraction (e.g. 0.003 = 0.3 %).
    pub fee_rate: f64,
    /// Total Value Locked in USD.
    pub tvl: f64,
    /// AMM mechanism type.
    pub pool_type: PoolType,
}

/// Default minimum TVL (in USD) used to filter out dust / empty pools.
pub const DEFAULT_MIN_TVL: f64 = 1_000.0;
