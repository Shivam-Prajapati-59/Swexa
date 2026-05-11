//! Resolves the on-chain account addresses needed by instruction builders.
//!
//! When we have a `PoolEdge` from the routing graph, we know the pool address
//! and token mints but not the vault addresses or other DEX-specific accounts.
//! This module fetches the pool's raw on-chain account data and deserializes
//! just enough fields to extract the vault pubkeys.
//!
//! The resolved addresses are then passed to the instruction builder to
//! construct the swap instruction for the lite-svm simulator.

use crate::engine::instruction_builder::SwapAccounts;
use crate::types::{DexProtocol, PoolEdge, PoolType};
use anyhow::{Result, anyhow};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::AccountMeta;
use std::collections::HashMap;

/// A cache of fetched account data keyed by address.
pub type AccountCache = HashMap<Address, Account>;

/// Resolves `SwapAccounts` for a pool hop by reading the pool's on-chain
/// account data and extracting vault pubkeys.
///
/// `user_authority` is the simulated payer's pubkey. The user token accounts
/// are ATAs derived from the mint + authority.
///
/// `input_mint` determines the direction of the swap.
pub fn resolve_swap_accounts(
    pool: &PoolEdge,
    input_mint: &str,
    user_authority: &Address,
    user_source_ata: &Address,
    user_dest_ata: &Address,
    account_cache: &AccountCache,
) -> Result<SwapAccounts> {
    let pool_address = parse_pool_address(&pool.address)?;

    let pool_account = account_cache
        .get(&pool_address)
        .ok_or_else(|| anyhow!("pool account not found in cache: {}", pool.address))?;

    match (pool.dex, pool.pool_type) {
        (DexProtocol::Raydium, PoolType::Amm) => {
            resolve_raydium_amm(pool_account, pool_address, user_authority, user_source_ata, user_dest_ata)
        }
        (DexProtocol::Raydium, PoolType::ConcentratedLiquidity) => {
            resolve_raydium_clmm(pool_account, pool_address, input_mint, pool, user_authority, user_source_ata, user_dest_ata)
        }
        (DexProtocol::Whirlpool, _) => {
            resolve_whirlpool(pool_account, pool_address, user_authority, user_source_ata, user_dest_ata)
        }
        (DexProtocol::Meteora, _) => {
            resolve_meteora_dlmm(pool_account, pool_address, user_authority, user_source_ata, user_dest_ata)
        }
        _ => Err(anyhow!(
            "unsupported dex/pool-type for account resolution: {:?}/{:?}",
            pool.dex,
            pool.pool_type
        )),
    }
}

/// Returns the list of on-chain pubkeys that need to be fetched for a pool
/// and its required accounts to be resolved. At minimum, this is the pool
/// state account. DEX-specific extras (oracles, tick arrays) can be added here.
pub fn required_accounts_for_pool(pool: &PoolEdge) -> Result<Vec<Address>> {
    let pool_address = parse_pool_address(&pool.address)?;
    // For now, we only need the pool state account itself.
    // The vault addresses are extracted from the pool state data.
    Ok(vec![pool_address])
}

// ── Raydium AMM V4 ────────────────────────────────────────────────────────

/// Raydium AMM V4 pool state layout offsets for vault pubkeys.
///
/// The AMM V4 layout stores vault pubkeys at known offsets:
///   - token_coin_vault (token A vault) at offset 200
///   - token_pc_vault   (token B vault) at offset 232
///   - amm_open_orders   at offset 136
///   - amm_target_orders at offset 168
/// These offsets are from the Raydium AMM V4 IDL specification.
fn resolve_raydium_amm(
    pool_account: &Account,
    pool_address: Address,
    user_authority: &Address,
    user_source_ata: &Address,
    user_dest_ata: &Address,
) -> Result<SwapAccounts> {
    let data = &pool_account.data;
    if data.len() < 264 {
        return Err(anyhow!(
            "raydium AMM account data too short: {} bytes (need ≥264)",
            data.len()
        ));
    }

    let token_a_vault = read_pubkey(data, 200)?;
    let token_b_vault = read_pubkey(data, 232)?;

    // Extra accounts for Raydium AMM: open_orders, target_orders
    let mut extra = Vec::new();
    if data.len() >= 200 {
        if let Ok(open_orders) = read_pubkey(data, 136) {
            extra.push(AccountMeta::new(open_orders, false));
        }
        if let Ok(target_orders) = read_pubkey(data, 168) {
            extra.push(AccountMeta::new(target_orders, false));
        }
    }

    Ok(SwapAccounts {
        pool_state: pool_address,
        token_a_vault,
        token_b_vault,
        user_source_token_account: *user_source_ata,
        user_destination_token_account: *user_dest_ata,
        user_authority: *user_authority,
        extra_accounts: extra,
    })
}

// ── Raydium CLMM ──────────────────────────────────────────────────────────

/// Raydium CLMM pool state layout:
///   - token_vault_0 at offset 73
///   - token_vault_1 at offset 105
///   - observation_key at offset 253
fn resolve_raydium_clmm(
    pool_account: &Account,
    pool_address: Address,
    _input_mint: &str,
    _pool: &PoolEdge,
    user_authority: &Address,
    user_source_ata: &Address,
    user_dest_ata: &Address,
) -> Result<SwapAccounts> {
    let data = &pool_account.data;
    if data.len() < 285 {
        return Err(anyhow!(
            "raydium CLMM account data too short: {} bytes (need ≥285)",
            data.len()
        ));
    }

    let token_a_vault = read_pubkey(data, 73)?;
    let token_b_vault = read_pubkey(data, 105)?;

    let mut extra = Vec::new();
    if let Ok(observation) = read_pubkey(data, 253) {
        extra.push(AccountMeta::new(observation, false));
    }

    Ok(SwapAccounts {
        pool_state: pool_address,
        token_a_vault,
        token_b_vault,
        user_source_token_account: *user_source_ata,
        user_destination_token_account: *user_dest_ata,
        user_authority: *user_authority,
        extra_accounts: extra,
    })
}

// ── Orca Whirlpool ────────────────────────────────────────────────────────

/// Whirlpool state layout:
///   - token_vault_a at offset 101
///   - token_vault_b at offset 133
///   - tick_arrays and oracle are passed as extra accounts
fn resolve_whirlpool(
    pool_account: &Account,
    pool_address: Address,
    user_authority: &Address,
    user_source_ata: &Address,
    user_dest_ata: &Address,
) -> Result<SwapAccounts> {
    let data = &pool_account.data;
    if data.len() < 165 {
        return Err(anyhow!(
            "whirlpool account data too short: {} bytes (need ≥165)",
            data.len()
        ));
    }

    let token_a_vault = read_pubkey(data, 101)?;
    let token_b_vault = read_pubkey(data, 133)?;

    // TODO(whirlpool): Whirlpool swap instructions require three tick_array accounts
    // and an oracle PDA as extra accounts. Deriving these requires:
    //   1. Reading the current tick_index from pool state (offset ~69, i32)
    //   2. Computing tick_array start indices based on tick_spacing
    //   3. Deriving tick_array PDAs: seeds = [b"tick_array", pool_pubkey, start_tick_index]
    //   4. Deriving oracle PDA: seeds = [b"oracle", pool_pubkey]
    // This is deferred because the simulator cannot yet execute the Whirlpool BPF
    // program, so these accounts are not needed for the heuristic fallback path.
    // When full BPF simulation is enabled, populate these PDAs here.
    let extra = Vec::new();

    Ok(SwapAccounts {
        pool_state: pool_address,
        token_a_vault,
        token_b_vault,
        user_source_token_account: *user_source_ata,
        user_destination_token_account: *user_dest_ata,
        user_authority: *user_authority,
        extra_accounts: extra,
    })
}

// ── Meteora DLMM ──────────────────────────────────────────────────────────

/// Meteora DLMM lb_pair layout:
///   - reserve_x at offset 72
///   - reserve_y at offset 104
///   - oracle at offset 168
fn resolve_meteora_dlmm(
    pool_account: &Account,
    pool_address: Address,
    user_authority: &Address,
    user_source_ata: &Address,
    user_dest_ata: &Address,
) -> Result<SwapAccounts> {
    let data = &pool_account.data;
    if data.len() < 200 {
        return Err(anyhow!(
            "meteora DLMM account data too short: {} bytes (need ≥200)",
            data.len()
        ));
    }

    let token_a_vault = read_pubkey(data, 72)?;
    let token_b_vault = read_pubkey(data, 104)?;

    let mut extra = Vec::new();
    if let Ok(oracle) = read_pubkey(data, 168) {
        extra.push(AccountMeta::new(oracle, false));
    }

    Ok(SwapAccounts {
        pool_state: pool_address,
        token_a_vault,
        token_b_vault,
        user_source_token_account: *user_source_ata,
        user_destination_token_account: *user_dest_ata,
        user_authority: *user_authority,
        extra_accounts: extra,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn read_pubkey(data: &[u8], offset: usize) -> Result<Address> {
    if offset + 32 > data.len() {
        return Err(anyhow!(
            "cannot read pubkey at offset {}: data is only {} bytes",
            offset,
            data.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&data[offset..offset + 32]);
    Ok(Address::from(arr))
}

fn parse_pool_address(base58: &str) -> Result<Address> {
    let bytes = bs58_decode(base58)?;
    if bytes.len() != 32 {
        return Err(anyhow!("invalid pool address length: expected 32, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Address::from(arr))
}

/// Minimal base58 decoder.
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

/// Derives an Associated Token Account (ATA) address deterministically.
///
/// ATA = PDA of [wallet, TOKEN_PROGRAM_ID, mint] under ATA_PROGRAM_ID.
/// For the simulator we just use a deterministic hash to create a unique
/// address per (wallet, mint) pair. The actual PDA derivation requires
/// sha256 + find_program_address which we simplify here since the simulator
/// only needs a consistent, unique address.
pub fn derive_simulated_ata(wallet: &Address, mint_str: &str) -> Address {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    wallet.to_bytes().hash(&mut hasher);
    mint_str.hash(&mut hasher);
    let hash = hasher.finish();

    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&hash.to_le_bytes());
    // Add a second hash for more entropy
    mint_str.as_bytes().hash(&mut hasher);
    let hash2 = hasher.finish();
    bytes[8..16].copy_from_slice(&hash2.to_le_bytes());
    // Fill remaining bytes deterministically
    wallet.to_bytes()[0..16].iter().enumerate().for_each(|(i, b)| {
        bytes[16 + i] = *b;
    });

    Address::from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_simulated_ata_is_deterministic() {
        let wallet = Address::from([42u8; 32]);
        let ata1 = derive_simulated_ata(&wallet, "SOL_MINT");
        let ata2 = derive_simulated_ata(&wallet, "SOL_MINT");
        assert_eq!(ata1, ata2);
    }

    #[test]
    fn derive_simulated_ata_differs_by_mint() {
        let wallet = Address::from([42u8; 32]);
        let ata_sol = derive_simulated_ata(&wallet, "SOL_MINT");
        let ata_usdc = derive_simulated_ata(&wallet, "USDC_MINT");
        assert_ne!(ata_sol, ata_usdc);
    }

    #[test]
    fn read_pubkey_at_offset() {
        let mut data = vec![0u8; 100];
        let expected = [7u8; 32];
        data[50..82].copy_from_slice(&expected);

        let result = read_pubkey(&data, 50).unwrap();
        assert_eq!(result.to_bytes(), expected);
    }

    #[test]
    fn read_pubkey_out_of_bounds() {
        let data = vec![0u8; 30];
        assert!(read_pubkey(&data, 0).is_err());
    }
}
