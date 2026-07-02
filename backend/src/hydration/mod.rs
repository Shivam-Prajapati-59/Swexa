use crate::models::pool::{ClmmTick, DlmmBin, PubkeyBytes};
use anyhow::{Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

pub const WHIRLPOOL_TICK_ARRAY_SIZE: i32 = 88;
const WHIRLPOOL_TICK_SIZE: usize = 113;
const WHIRLPOOL_TICK_ARRAY_HEADER_SIZE: usize = 12;
pub const METEORA_BINS_PER_ARRAY: i32 = 70;
const METEORA_BIN_ARRAY_DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
const METEORA_BIN_ARRAY_HEADER_SIZE: usize = 56;
const METEORA_BIN_SIZE: usize = 144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenVaultAmount {
    pub mint: PubkeyBytes,
    pub amount: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhirlpoolAccountState {
    pub tick_spacing: u16,
    pub fee_rate: u32,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub current_tick_index: i32,
    pub token_mint_a: PubkeyBytes,
    pub token_vault_a: PubkeyBytes,
    pub token_mint_b: PubkeyBytes,
    pub token_vault_b: PubkeyBytes,
}

#[inline]
pub fn pubkey_bytes_to_pubkey(pubkey: PubkeyBytes) -> Pubkey {
    Pubkey::new_from_array(pubkey.0)
}

#[inline]
pub fn pubkey_to_bytes(pubkey: Pubkey) -> PubkeyBytes {
    PubkeyBytes(pubkey.to_bytes())
}

pub async fn fetch_accounts(
    rpc: &RpcClient,
    pubkeys: &[PubkeyBytes],
) -> Result<Vec<Option<Vec<u8>>>> {
    let pubkeys: Vec<Pubkey> = pubkeys
        .iter()
        .copied()
        .map(pubkey_bytes_to_pubkey)
        .collect();
    let accounts = rpc
        .get_multiple_accounts(&pubkeys)
        .await
        .context("failed to fetch RPC accounts")?;

    Ok(accounts
        .into_iter()
        .map(|account| account.map(|account| account.data))
        .collect())
}

pub fn parse_token_vault_amount(data: &[u8]) -> Result<TokenVaultAmount> {
    if data.len() < 72 {
        anyhow::bail!("token account data too short: {}", data.len());
    }

    let mint = read_pubkey(data, 0)?;
    let amount = read_u64(data, 64)? as u128;
    Ok(TokenVaultAmount { mint, amount })
}

pub fn parse_whirlpool_account(data: &[u8]) -> Result<WhirlpoolAccountState> {
    if data.len() < 245 {
        anyhow::bail!("whirlpool account data too short: {}", data.len());
    }

    // Whirlpool is an Anchor/Borsh account. Offsets are the serialized field
    // order after the 8-byte discriminator; Borsh adds no alignment padding.
    Ok(WhirlpoolAccountState {
        tick_spacing: read_u16(data, 41)?,
        fee_rate: read_u16(data, 45)? as u32,
        liquidity: read_u128(data, 49)?,
        sqrt_price_x64: read_u128(data, 65)?,
        current_tick_index: read_i32(data, 81)?,
        token_mint_a: read_pubkey(data, 101)?,
        token_vault_a: read_pubkey(data, 133)?,
        token_mint_b: read_pubkey(data, 181)?,
        token_vault_b: read_pubkey(data, 213)?,
    })
}

pub fn whirlpool_tick_array_start_tick(tick_index: i32, tick_spacing: u16) -> i32 {
    let ticks_per_array = WHIRLPOOL_TICK_ARRAY_SIZE * tick_spacing as i32;
    tick_index.div_euclid(ticks_per_array) * ticks_per_array
}

pub fn derive_whirlpool_tick_array_pda(
    whirlpool: PubkeyBytes,
    start_tick_index: i32,
) -> Result<PubkeyBytes> {
    let program_id = Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct5wRPh2YpFb4fo")
        .context("invalid Whirlpool program id")?;
    let whirlpool = pubkey_bytes_to_pubkey(whirlpool);
    let start_tick_index = start_tick_index.to_string();
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"tick_array",
            whirlpool.as_ref(),
            start_tick_index.as_bytes(),
        ],
        &program_id,
    );
    Ok(pubkey_to_bytes(pda))
}

pub fn parse_whirlpool_tick_array(data: &[u8], tick_spacing: u16) -> Result<Vec<ClmmTick>> {
    let min_len = WHIRLPOOL_TICK_ARRAY_HEADER_SIZE + WHIRLPOOL_TICK_SIZE * 88;
    if data.len() < min_len {
        anyhow::bail!("whirlpool tick array data too short: {}", data.len());
    }

    let start_tick_index = read_i32(data, 8)?;
    let mut ticks = Vec::new();
    for i in 0..88usize {
        let offset = WHIRLPOOL_TICK_ARRAY_HEADER_SIZE + i * WHIRLPOOL_TICK_SIZE;
        let initialized = *data
            .get(offset)
            .with_context(|| format!("tick array missing initialized byte at {offset}"))?;
        if initialized == 0 {
            continue;
        }

        let liquidity_net = read_i128(data, offset + 1)?;
        ticks.push(ClmmTick {
            index: start_tick_index + i as i32 * tick_spacing as i32,
            liquidity_net,
        });
    }
    Ok(ticks)
}

pub fn parse_meteora_active_id(data: &[u8]) -> Option<i32> {
    // LbPair is a bytemuck account. active_id sits after the 8-byte Anchor
    // discriminator, StaticParameters(32), VariableParameters(32), bump_seed(1),
    // bin_step_seed(2), and pair_type(1).
    read_i32(data, 76).ok()
}

pub fn meteora_bin_array_index(bin_id: i32) -> i64 {
    (bin_id as i64).div_euclid(METEORA_BINS_PER_ARRAY as i64)
}

pub fn derive_meteora_bin_array_pda(lb_pair: PubkeyBytes, index: i64) -> Result<PubkeyBytes> {
    let program_id = Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo")
        .context("invalid Meteora DLMM program id")?;
    let lb_pair = pubkey_bytes_to_pubkey(lb_pair);
    let (pda, _) = Pubkey::find_program_address(
        &[b"bin_array", lb_pair.as_ref(), &index.to_le_bytes()],
        &program_id,
    );
    Ok(pubkey_to_bytes(pda))
}

pub fn parse_meteora_bin_array(data: &[u8], expected_lb_pair: PubkeyBytes) -> Result<Vec<DlmmBin>> {
    let min_len =
        METEORA_BIN_ARRAY_HEADER_SIZE + METEORA_BIN_SIZE * METEORA_BINS_PER_ARRAY as usize;
    if data.len() < min_len {
        anyhow::bail!("meteora bin array data too short: {}", data.len());
    }
    if data.get(0..8) != Some(METEORA_BIN_ARRAY_DISCRIMINATOR.as_slice()) {
        anyhow::bail!("meteora bin array discriminator mismatch");
    }

    let array_index = read_i64(data, 8)?;
    let lb_pair = read_pubkey(data, 24)?;
    if lb_pair != expected_lb_pair {
        anyhow::bail!("meteora bin array parent mismatch");
    }

    let mut bins = Vec::new();
    for i in 0..METEORA_BINS_PER_ARRAY as usize {
        let offset = METEORA_BIN_ARRAY_HEADER_SIZE + i * METEORA_BIN_SIZE;
        let amount_x = read_u64(data, offset)? as u128;
        let amount_y = read_u64(data, offset + 8)? as u128;
        if amount_x == 0 && amount_y == 0 {
            continue;
        }

        let id = array_index
            .checked_mul(METEORA_BINS_PER_ARRAY as i64)
            .and_then(|start| start.checked_add(i as i64))
            .and_then(|id| i32::try_from(id).ok())
            .ok_or_else(|| anyhow::anyhow!("meteora bin id out of i32 range"))?;

        bins.push(DlmmBin {
            id,
            amount_x,
            amount_y,
        });
    }

    Ok(bins)
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .context("offset overflow while reading account data")?;
    let bytes = data
        .get(offset..end)
        .with_context(|| format!("account data too short at offset {offset}"))?;
    Ok(bytes.try_into().expect("slice length checked"))
}

fn read_pubkey(data: &[u8], offset: usize) -> Result<PubkeyBytes> {
    Ok(PubkeyBytes(read_array::<32>(data, offset)?))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array::<2>(data, offset)?))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array::<8>(data, offset)?))
}

fn read_u128(data: &[u8], offset: usize) -> Result<u128> {
    Ok(u128::from_le_bytes(read_array::<16>(data, offset)?))
}

fn read_i128(data: &[u8], offset: usize) -> Result<i128> {
    Ok(i128::from_le_bytes(read_array::<16>(data, offset)?))
}

fn read_i32(data: &[u8], offset: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(read_array::<4>(data, offset)?))
}

fn read_i64(data: &[u8], offset: usize) -> Result<i64> {
    Ok(i64::from_le_bytes(read_array::<8>(data, offset)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bytes(data: &mut [u8], offset: usize, bytes: &[u8]) {
        data[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    #[test]
    fn parses_token_vault_amount() {
        let mint = PubkeyBytes([7u8; 32]);
        let mut data = vec![0u8; 165];
        write_bytes(&mut data, 0, &mint.0);
        write_bytes(&mut data, 64, &123_456u64.to_le_bytes());

        let parsed = parse_token_vault_amount(&data).unwrap();

        assert_eq!(parsed.mint, mint);
        assert_eq!(parsed.amount, 123_456);
    }

    #[test]
    fn parses_whirlpool_account_core_fields() {
        let mint_a = PubkeyBytes([1u8; 32]);
        let vault_a = PubkeyBytes([2u8; 32]);
        let mint_b = PubkeyBytes([3u8; 32]);
        let vault_b = PubkeyBytes([4u8; 32]);
        let mut data = vec![0u8; 245];
        write_bytes(&mut data, 41, &4u16.to_le_bytes());
        write_bytes(&mut data, 45, &400u16.to_le_bytes());
        write_bytes(&mut data, 49, &987_654_321u128.to_le_bytes());
        write_bytes(&mut data, 65, &(1u128 << 64).to_le_bytes());
        write_bytes(&mut data, 81, &(-123i32).to_le_bytes());
        write_bytes(&mut data, 101, &mint_a.0);
        write_bytes(&mut data, 133, &vault_a.0);
        write_bytes(&mut data, 181, &mint_b.0);
        write_bytes(&mut data, 213, &vault_b.0);

        let parsed = parse_whirlpool_account(&data).unwrap();

        assert_eq!(parsed.tick_spacing, 4);
        assert_eq!(parsed.fee_rate, 400);
        assert_eq!(parsed.liquidity, 987_654_321);
        assert_eq!(parsed.sqrt_price_x64, 1u128 << 64);
        assert_eq!(parsed.current_tick_index, -123);
        assert_eq!(parsed.token_mint_a, mint_a);
        assert_eq!(parsed.token_vault_a, vault_a);
        assert_eq!(parsed.token_mint_b, mint_b);
        assert_eq!(parsed.token_vault_b, vault_b);
    }

    #[test]
    fn parses_initialized_whirlpool_ticks() {
        let mut data = vec![0u8; WHIRLPOOL_TICK_ARRAY_HEADER_SIZE + WHIRLPOOL_TICK_SIZE * 88];
        write_bytes(&mut data, 8, &(-176i32).to_le_bytes());
        let tick_offset = WHIRLPOOL_TICK_ARRAY_HEADER_SIZE + 3 * WHIRLPOOL_TICK_SIZE;
        data[tick_offset] = 1;
        write_bytes(&mut data, tick_offset + 1, &123i128.to_le_bytes());

        let ticks = parse_whirlpool_tick_array(&data, 4).unwrap();

        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].index, -164);
        assert_eq!(ticks[0].liquidity_net, 123);
    }

    #[test]
    fn meteora_bin_array_index_floors_negative_ids() {
        assert_eq!(meteora_bin_array_index(0), 0);
        assert_eq!(meteora_bin_array_index(69), 0);
        assert_eq!(meteora_bin_array_index(70), 1);
        assert_eq!(meteora_bin_array_index(-1), -1);
        assert_eq!(meteora_bin_array_index(-70), -1);
        assert_eq!(meteora_bin_array_index(-71), -2);
    }

    #[test]
    fn parses_meteora_bin_array_liquidity_bins() {
        let lb_pair = PubkeyBytes([9u8; 32]);
        let mut data = vec![0u8; METEORA_BIN_ARRAY_HEADER_SIZE + METEORA_BIN_SIZE * 70];
        write_bytes(&mut data, 0, &METEORA_BIN_ARRAY_DISCRIMINATOR);
        write_bytes(&mut data, 8, &(-2i64).to_le_bytes());
        data[16] = 2;
        write_bytes(&mut data, 24, &lb_pair.0);

        let bin_offset = METEORA_BIN_ARRAY_HEADER_SIZE + 69 * METEORA_BIN_SIZE;
        write_bytes(&mut data, bin_offset, &11u64.to_le_bytes());
        write_bytes(&mut data, bin_offset + 8, &22u64.to_le_bytes());

        let bins = parse_meteora_bin_array(&data, lb_pair).unwrap();

        assert_eq!(bins.len(), 1);
        assert_eq!(bins[0].id, -71);
        assert_eq!(bins[0].amount_x, 11);
        assert_eq!(bins[0].amount_y, 22);
    }
}
