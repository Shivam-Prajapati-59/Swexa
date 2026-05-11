//! Builds Solana instructions for DEX swaps.
//!
//! Each supported DEX has its own on-chain program with a unique instruction
//! layout. This module provides a unified interface that takes a `PoolEdge`
//! (the protocol-agnostic routing abstraction) and produces the concrete
//! `Instruction` vector needed to perform that swap in the lite-svm simulator.
//!
//! ## Design Notes
//!
//! We use **raw instruction construction** (not SDK/CPI helpers) because:
//!  - The simulator only needs the instruction bytes, not actual CPI context
//!  - It avoids pulling in heavy DEX SDK crates
//!  - Account addresses for each pool are derived from the on-chain pool state
//!    that we already fetch via `AccountFetcher`
//!
//! The instruction data layout follows each DEX's published IDL / program spec.

use crate::config::dex::{
    METEORA_DLMM_PROGRAM_ID, RAYDIUM_AMM_PROGRAM_ID, RAYDIUM_CLMM_PROGRAM_ID,
    TOKEN_PROGRAM_ID, WHIRLPOOL_PROGRAM_ID,
};
use crate::types::{DexProtocol, PoolEdge, PoolType};
use anyhow::{Result, anyhow};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};

/// Direction of a swap relative to the pool's token_a / token_b ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapDirection {
    /// Swapping token_a → token_b (a_to_b = true).
    AtoB,
    /// Swapping token_b → token_a (a_to_b = false).
    BtoA,
}

/// All the on-chain account addresses the instruction builder needs for a
/// single hop. These are resolved by `AccountResolver` from the raw pool
/// account data fetched via RPC.
#[derive(Debug, Clone)]
pub struct SwapAccounts {
    /// The on-chain pool / market state account.
    pub pool_state: Address,
    /// The pool's token A vault.
    pub token_a_vault: Address,
    /// The pool's token B vault.
    pub token_b_vault: Address,
    /// The user's (simulator payer's) source token account.
    pub user_source_token_account: Address,
    /// The user's (simulator payer's) destination token account.
    pub user_destination_token_account: Address,
    /// The user/authority that owns the source token account.
    pub user_authority: Address,
    /// DEX-specific extra accounts (e.g. oracle, tick arrays, observation).
    /// Order matters — they are appended after the common accounts.
    pub extra_accounts: Vec<AccountMeta>,
}

/// Builds swap instructions for a single hop, dispatching to the correct DEX.
pub fn build_swap_instruction(
    pool: &PoolEdge,
    direction: SwapDirection,
    amount_in: u64,
    minimum_amount_out: u64,
    accounts: &SwapAccounts,
) -> Result<Vec<Instruction>> {
    match (pool.dex, pool.pool_type) {
        (DexProtocol::Raydium, PoolType::Amm) => {
            build_raydium_amm_swap(direction, amount_in, minimum_amount_out, accounts)
        }
        (DexProtocol::Raydium, PoolType::ConcentratedLiquidity) => {
            build_raydium_clmm_swap(direction, amount_in, minimum_amount_out, accounts)
        }
        (DexProtocol::Whirlpool, _) => {
            build_whirlpool_swap(direction, amount_in, minimum_amount_out, accounts)
        }
        (DexProtocol::Meteora, _) => {
            build_meteora_dlmm_swap(direction, amount_in, minimum_amount_out, accounts)
        }
        _ => Err(anyhow!(
            "unsupported DEX/pool-type combination: {:?}/{:?}",
            pool.dex,
            pool.pool_type
        )),
    }
}

/// Determines the swap direction for a hop given the current input mint.
///
/// Returns `Err` if `input_mint` matches neither token_a nor token_b.
pub fn resolve_direction(pool: &PoolEdge, input_mint: &str) -> Result<SwapDirection> {
    if pool.token_a.mint == input_mint {
        Ok(SwapDirection::AtoB)
    } else if pool.token_b.mint == input_mint {
        Ok(SwapDirection::BtoA)
    } else {
        Err(anyhow!(
            "input_mint '{}' does not match pool {} tokens (a='{}', b='{}')",
            input_mint,
            pool.address,
            pool.token_a.mint,
            pool.token_b.mint
        ))
    }
}

/// Returns the output mint for a hop given the input mint.
///
/// Returns `Err` if `input_mint` matches neither token_a nor token_b.
pub fn output_mint_for_hop<'a>(pool: &'a PoolEdge, input_mint: &str) -> Result<&'a str> {
    if pool.token_a.mint == input_mint {
        Ok(&pool.token_b.mint)
    } else if pool.token_b.mint == input_mint {
        Ok(&pool.token_a.mint)
    } else {
        Err(anyhow!(
            "input_mint '{}' does not match pool {} tokens (a='{}', b='{}')",
            input_mint,
            pool.address,
            pool.token_a.mint,
            pool.token_b.mint
        ))
    }
}

// ── Raydium AMM V4 ────────────────────────────────────────────────────────

/// Raydium AMM V4 swap instruction (discriminator = 9).
///
/// Instruction data layout (little-endian):
/// ```text
///   [0]       u8    discriminator = 9  (swap)
///   [1..9]    u64   amount_in
///   [9..17]   u64   minimum_amount_out
/// ```
fn build_raydium_amm_swap(
    direction: SwapDirection,
    amount_in: u64,
    minimum_amount_out: u64,
    accounts: &SwapAccounts,
) -> Result<Vec<Instruction>> {
    let program_id = parse_address(RAYDIUM_AMM_PROGRAM_ID)?;
    let token_program = parse_address(TOKEN_PROGRAM_ID)?;

    let mut data = Vec::with_capacity(17);
    data.push(9u8); // swap discriminator
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    // Account ordering follows the Raydium AMM V4 swap instruction spec.
    // The source/destination vaults are swapped depending on direction.
    let (source_vault, dest_vault) = match direction {
        SwapDirection::AtoB => (accounts.token_a_vault, accounts.token_b_vault),
        SwapDirection::BtoA => (accounts.token_b_vault, accounts.token_a_vault),
    };

    let mut account_metas = vec![
        AccountMeta::new(accounts.pool_state, false),        // amm
        AccountMeta::new_readonly(accounts.user_authority, true), // authority (signer)
        AccountMeta::new(accounts.user_source_token_account, false), // user source
        AccountMeta::new(accounts.user_destination_token_account, false), // user dest
        AccountMeta::new(source_vault, false),                // pool source vault
        AccountMeta::new(dest_vault, false),                  // pool dest vault
        AccountMeta::new_readonly(token_program, false),      // token program
    ];
    account_metas.extend_from_slice(&accounts.extra_accounts);

    Ok(vec![Instruction {
        program_id,
        accounts: account_metas,
        data,
    }])
}

// ── Raydium CLMM ──────────────────────────────────────────────────────────

/// Raydium CLMM swap instruction.
///
/// Uses the Anchor-style 8-byte discriminator for `swap`.
/// Instruction data layout:
/// ```text
///   [0..8]    [u8; 8]  anchor discriminator for "global:swap"
///   [8..16]   u64      amount (amount_in)
///   [16..24]  u64      other_amount_threshold (minimum_amount_out)
///   [24..40]  u128     sqrt_price_limit_x64 (0 = no limit)
///   [40]      bool     is_base_input (a_to_b)
/// ```
fn build_raydium_clmm_swap(
    direction: SwapDirection,
    amount_in: u64,
    minimum_amount_out: u64,
    accounts: &SwapAccounts,
) -> Result<Vec<Instruction>> {
    let program_id = parse_address(RAYDIUM_CLMM_PROGRAM_ID)?;
    let token_program = parse_address(TOKEN_PROGRAM_ID)?;

    // Anchor discriminator for Raydium CLMM "global:swap"
    // = sha256("global:swap")[..8] under the Raydium CLMM program namespace
    let discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];

    let a_to_b = direction == SwapDirection::AtoB;

    // sqrt_price_limit_x64: 0 means no price limit
    let sqrt_price_limit_x64: u128 = 0;

    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&discriminator);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit_x64.to_le_bytes());
    data.push(a_to_b as u8);

    let (source_vault, dest_vault) = match direction {
        SwapDirection::AtoB => (accounts.token_a_vault, accounts.token_b_vault),
        SwapDirection::BtoA => (accounts.token_b_vault, accounts.token_a_vault),
    };

    let mut account_metas = vec![
        AccountMeta::new_readonly(accounts.user_authority, true), // payer / signer
        AccountMeta::new(accounts.pool_state, false),        // pool state
        AccountMeta::new(accounts.user_source_token_account, false), // user source ata
        AccountMeta::new(accounts.user_destination_token_account, false), // user dest ata
        AccountMeta::new(source_vault, false),                // input vault
        AccountMeta::new(dest_vault, false),                  // output vault
        AccountMeta::new_readonly(token_program, false),      // token program
    ];
    account_metas.extend_from_slice(&accounts.extra_accounts);

    Ok(vec![Instruction {
        program_id,
        accounts: account_metas,
        data,
    }])
}

// ── Orca Whirlpool ────────────────────────────────────────────────────────

/// Orca Whirlpool swap instruction.
///
/// Uses Anchor-style discriminator for "swap".
/// Instruction data layout:
/// ```text
///   [0..8]    [u8; 8]  anchor discriminator for "swap"
///   [8..16]   u64      amount
///   [16..24]  u64      other_amount_threshold (minimum_amount_out)
///   [24..40]  u128     sqrt_price_limit (0 = no limit)
///   [40]      bool     amount_specified_is_input (true)
///   [41]      bool     a_to_b
/// ```
fn build_whirlpool_swap(
    direction: SwapDirection,
    amount_in: u64,
    minimum_amount_out: u64,
    accounts: &SwapAccounts,
) -> Result<Vec<Instruction>> {
    let program_id = parse_address(WHIRLPOOL_PROGRAM_ID)?;
    let token_program = parse_address(TOKEN_PROGRAM_ID)?;

    // Anchor discriminator for Orca Whirlpool "global:swap"
    // = sha256("global:swap")[..8] under the Whirlpool program namespace
    // Whirlpool uses a different IDL namespace than Raydium CLMM
    let discriminator: [u8; 8] = [43, 4, 237, 11, 26, 201, 30, 190];

    let a_to_b = direction == SwapDirection::AtoB;

    // sqrt_price_limit: use min/max to express "no limit"
    let sqrt_price_limit: u128 = if a_to_b {
        4295048016u128 // MIN_SQRT_PRICE
    } else {
        79226673515401279992447579055u128 // MAX_SQRT_PRICE
    };

    let mut data = Vec::with_capacity(42);
    data.extend_from_slice(&discriminator);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    data.push(1u8); // amount_specified_is_input = true
    data.push(a_to_b as u8);

    let (source_vault, dest_vault) = match direction {
        SwapDirection::AtoB => (accounts.token_a_vault, accounts.token_b_vault),
        SwapDirection::BtoA => (accounts.token_b_vault, accounts.token_a_vault),
    };

    let mut account_metas = vec![
        AccountMeta::new_readonly(token_program, false),      // token program
        AccountMeta::new_readonly(accounts.user_authority, true), // token authority (signer)
        AccountMeta::new(accounts.pool_state, false),         // whirlpool
        AccountMeta::new(accounts.user_source_token_account, false), // token owner acct A/B
        AccountMeta::new(source_vault, false),                // token vault A/B
        AccountMeta::new(accounts.user_destination_token_account, false), // token owner acct B/A
        AccountMeta::new(dest_vault, false),                  // token vault B/A
    ];
    account_metas.extend_from_slice(&accounts.extra_accounts);

    Ok(vec![Instruction {
        program_id,
        accounts: account_metas,
        data,
    }])
}

// ── Meteora DLMM ──────────────────────────────────────────────────────────

/// Meteora DLMM swap instruction.
///
/// Instruction data layout:
/// ```text
///   [0..8]    [u8; 8]  anchor discriminator for "swap"
///   [8..16]   u64      amount_in
///   [16..24]  u64      minimum_amount_out
/// ```
fn build_meteora_dlmm_swap(
    direction: SwapDirection,
    amount_in: u64,
    minimum_amount_out: u64,
    accounts: &SwapAccounts,
) -> Result<Vec<Instruction>> {
    let program_id = parse_address(METEORA_DLMM_PROGRAM_ID)?;
    let token_program = parse_address(TOKEN_PROGRAM_ID)?;

    // Anchor discriminator for Meteora DLMM "swap"
    // = sha256("global:swap")[..8] under the Meteora DLMM program namespace
    let discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];

    let a_to_b = direction == SwapDirection::AtoB;

    // Layout: discriminator (8) + amount_in (8) + min_out (8) + direction (1) = 25 bytes
    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&discriminator);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.push(a_to_b as u8);

    let (source_vault, dest_vault) = match direction {
        SwapDirection::AtoB => (accounts.token_a_vault, accounts.token_b_vault),
        SwapDirection::BtoA => (accounts.token_b_vault, accounts.token_a_vault),
    };

    let mut account_metas = vec![
        AccountMeta::new(accounts.pool_state, false),         // lb_pair
        AccountMeta::new_readonly(accounts.user_authority, true), // user (signer)
        AccountMeta::new(accounts.user_source_token_account, false), // user source
        AccountMeta::new(accounts.user_destination_token_account, false), // user dest
        AccountMeta::new(source_vault, false),                // reserve source
        AccountMeta::new(dest_vault, false),                  // reserve dest
        AccountMeta::new_readonly(token_program, false),      // token program
    ];
    account_metas.extend_from_slice(&accounts.extra_accounts);

    Ok(vec![Instruction {
        program_id,
        accounts: account_metas,
        data,
    }])
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn parse_address(base58: &str) -> Result<Address> {
    let bytes = bs58_decode(base58)?;
    if bytes.len() != 32 {
        return Err(anyhow!("invalid address length: expected 32, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Address::from(arr))
}

/// Minimal base58 decoder (avoids pulling in `bs58` crate).
fn bs58_decode(input: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    let mut result = vec![0u8; 0];
    let mut leading_zeros = 0;

    for ch in input.bytes() {
        if ch == b'1' && result.is_empty() {
            leading_zeros += 1;
            continue;
        }

        let digit = ALPHABET
            .iter()
            .position(|&c| c == ch)
            .ok_or_else(|| anyhow!("invalid base58 character: {}", ch as char))?;

        // Multiply existing result by 58 and add digit
        let mut carry = digit;
        for byte in result.iter_mut().rev() {
            let v = (*byte as usize) * 58 + carry;
            *byte = (v % 256) as u8;
            carry = v / 256;
        }
        while carry > 0 {
            result.insert(0, (carry % 256) as u8);
            carry /= 256;
        }
    }

    let mut output = vec![0u8; leading_zeros];
    output.extend(result);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_program_ids() {
        // Smoke test: all program IDs parse without error
        assert!(parse_address(RAYDIUM_AMM_PROGRAM_ID).is_ok());
        assert!(parse_address(RAYDIUM_CLMM_PROGRAM_ID).is_ok());
        assert!(parse_address(WHIRLPOOL_PROGRAM_ID).is_ok());
        assert!(parse_address(METEORA_DLMM_PROGRAM_ID).is_ok());
        assert!(parse_address(TOKEN_PROGRAM_ID).is_ok());
    }

    #[test]
    fn resolve_direction_a_to_b() {
        let pool = test_pool("SOL", "USDC");
        assert_eq!(resolve_direction(&pool, "SOL").unwrap(), SwapDirection::AtoB);
        assert_eq!(resolve_direction(&pool, "USDC").unwrap(), SwapDirection::BtoA);
    }

    #[test]
    fn resolve_direction_unknown_mint_errors() {
        let pool = test_pool("SOL", "USDC");
        assert!(resolve_direction(&pool, "BONK").is_err());
    }

    #[test]
    fn output_mint_follows_direction() {
        let pool = test_pool("SOL", "USDC");
        assert_eq!(output_mint_for_hop(&pool, "SOL").unwrap(), "USDC");
        assert_eq!(output_mint_for_hop(&pool, "USDC").unwrap(), "SOL");
    }

    #[test]
    fn output_mint_unknown_mint_errors() {
        let pool = test_pool("SOL", "USDC");
        assert!(output_mint_for_hop(&pool, "BONK").is_err());
    }

    #[test]
    fn raydium_amm_instruction_data_is_17_bytes() {
        let accounts = dummy_accounts();
        let instructions =
            build_raydium_amm_swap(SwapDirection::AtoB, 1000, 900, &accounts).unwrap();
        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].data.len(), 17);
        assert_eq!(instructions[0].data[0], 9); // discriminator
    }

    fn test_pool(a: &str, b: &str) -> PoolEdge {
        PoolEdge {
            address: "pool_address".to_string(),
            dex: DexProtocol::Raydium,
            token_a: crate::types::TokenMint {
                mint: a.to_string(),
                symbol: a.to_string(),
                decimals: 9,
            },
            token_b: crate::types::TokenMint {
                mint: b.to_string(),
                symbol: b.to_string(),
                decimals: 9,
            },
            fee_rate: 0.003,
            tvl: 100_000.0,
            pool_type: PoolType::Amm,
        }
    }

    fn dummy_accounts() -> SwapAccounts {
        SwapAccounts {
            pool_state: Address::from([1u8; 32]),
            token_a_vault: Address::from([2u8; 32]),
            token_b_vault: Address::from([3u8; 32]),
            user_source_token_account: Address::from([4u8; 32]),
            user_destination_token_account: Address::from([5u8; 32]),
            user_authority: Address::from([6u8; 32]),
            extra_accounts: vec![],
        }
    }
}
