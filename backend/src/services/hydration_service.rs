use crate::graph::builder::RouteCandidate;
use crate::hydration::{
    derive_meteora_bin_array_pda, derive_whirlpool_tick_array_pda, fetch_accounts,
    meteora_bin_array_index, parse_meteora_active_id, parse_meteora_bin_array,
    parse_token_vault_amount, parse_whirlpool_account, parse_whirlpool_tick_array,
    whirlpool_tick_array_start_tick,
};
use crate::models::pool::{DexProtocol, Pool, PoolData, PoolId, PubkeyBytes};
use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::collections::{BTreeSet, HashMap};

const WHIRLPOOL_TICK_ARRAY_RADIUS: i32 = 2;
const METEORA_BIN_ARRAY_RADIUS: i64 = 3;

pub async fn hydrate_candidate_pools(
    pools: &[Pool],
    candidates: &[RouteCandidate],
    rpc: &RpcClient,
) -> Vec<Pool> {
    let target_ids = candidate_pool_ids(candidates);
    if target_ids.is_empty() {
        return Vec::new();
    }

    let pool_by_id: HashMap<PoolId, &Pool> =
        pools.iter().map(|pool| (pool.pool_id(), pool)).collect();
    let mut hydrated = Vec::with_capacity(target_ids.len());

    for pool_id in target_ids {
        let Some(pool) = pool_by_id.get(&pool_id).copied() else {
            continue;
        };

        let hydrated_pool = match pool.metadata.protocol {
            DexProtocol::Whirlpool => hydrate_whirlpool(pool.clone(), rpc).await,
            DexProtocol::Meteora => hydrate_meteora(pool.clone(), rpc).await,
            _ => Ok(pool.clone()),
        };

        match hydrated_pool {
            Ok(pool) => hydrated.push(pool),
            Err(error) => log::warn!("Pool hydration failed for pool {pool_id}: {error:#}"),
        }
    }

    hydrated
}

fn candidate_pool_ids(candidates: &[RouteCandidate]) -> BTreeSet<PoolId> {
    candidates
        .iter()
        .flat_map(|route| route.steps.iter().map(|step| step.pool_id))
        .collect()
}

async fn hydrate_whirlpool(mut pool: Pool, rpc: &RpcClient) -> Result<Pool> {
    let token_a_mint = pool.metadata.token_a.mint;
    let token_b_mint = pool.metadata.token_b.mint;
    let Some(token_a_vault) = pool.metadata.token_a.vault else {
        return Ok(pool);
    };
    let Some(token_b_vault) = pool.metadata.token_b.vault else {
        return Ok(pool);
    };

    let PoolData::Clmm(state) = &mut pool.data else {
        return Ok(pool);
    };

    let accounts =
        fetch_accounts(rpc, &[pool.metadata.pubkey, token_a_vault, token_b_vault]).await?;

    if let [Some(pool_data), Some(vault_a_data), Some(vault_b_data)] = accounts.as_slice() {
        let onchain = parse_whirlpool_account(pool_data)?;
        let matches_api = onchain.token_mint_a == token_a_mint
            && onchain.token_mint_b == token_b_mint
            && onchain.token_vault_a == token_a_vault
            && onchain.token_vault_b == token_b_vault;

        if matches_api {
            state.liquidity = Some(onchain.liquidity);
            state.sqrt_price_x64 = Some(onchain.sqrt_price_x64);
            state.current_tick_index = Some(onchain.current_tick_index);
            state.tick_spacing = onchain.tick_spacing;
            pool.fee_rate = onchain.fee_rate;
        }

        if let Ok(vault) = parse_token_vault_amount(vault_a_data)
            && vault.mint == token_a_mint
        {
            state.reserve_a = Some(vault.amount);
        }
        if let Ok(vault) = parse_token_vault_amount(vault_b_data)
            && vault.mint == token_b_mint
        {
            state.reserve_b = Some(vault.amount);
        }
    }

    let Some(current_tick_index) = state.current_tick_index else {
        return Ok(pool);
    };

    let start_tick = whirlpool_tick_array_start_tick(current_tick_index, state.tick_spacing);
    let ticks_per_array = 88 * state.tick_spacing as i32;
    let tick_array_pubkeys: Vec<PubkeyBytes> = (-WHIRLPOOL_TICK_ARRAY_RADIUS
        ..=WHIRLPOOL_TICK_ARRAY_RADIUS)
        .filter_map(|offset| {
            let start = start_tick.checked_add(offset * ticks_per_array)?;
            derive_whirlpool_tick_array_pda(pool.metadata.pubkey, start).ok()
        })
        .collect();

    let accounts = fetch_accounts(rpc, &tick_array_pubkeys).await?;
    state.initialized_ticks.clear();
    for data in accounts.into_iter().flatten() {
        match parse_whirlpool_tick_array(&data, state.tick_spacing) {
            Ok(mut ticks) => state.initialized_ticks.append(&mut ticks),
            Err(error) => log::debug!(
                "Whirlpool tick array parse failed for pool {}: {error:#}",
                pool.metadata.id
            ),
        }
    }
    state.initialized_ticks.sort_by_key(|tick| tick.index);
    state.initialized_ticks.dedup_by_key(|tick| tick.index);

    Ok(pool)
}

async fn hydrate_meteora(mut pool: Pool, rpc: &RpcClient) -> Result<Pool> {
    let token_a_mint = pool.metadata.token_a.mint;
    let token_b_mint = pool.metadata.token_b.mint;
    let Some(token_a_vault) = pool.metadata.token_a.vault else {
        return Ok(pool);
    };
    let Some(token_b_vault) = pool.metadata.token_b.vault else {
        return Ok(pool);
    };

    let PoolData::Dlmm(state) = &mut pool.data else {
        return Ok(pool);
    };

    let accounts =
        fetch_accounts(rpc, &[pool.metadata.pubkey, token_a_vault, token_b_vault]).await?;

    if let [pool_account, vault_a_account, vault_b_account] = accounts.as_slice() {
        if let Some(pool_data) = pool_account {
            state.active_bin_id = parse_meteora_active_id(pool_data);
        }

        if let Some(vault_a_data) = vault_a_account
            && let Ok(vault) = parse_token_vault_amount(vault_a_data)
            && vault.mint == token_a_mint
        {
            state.reserve_a = Some(vault.amount);
        }
        if let Some(vault_b_data) = vault_b_account
            && let Ok(vault) = parse_token_vault_amount(vault_b_data)
            && vault.mint == token_b_mint
        {
            state.reserve_b = Some(vault.amount);
        }
    }

    let Some(active_bin_id) = state.active_bin_id else {
        return Ok(pool);
    };

    let active_array_index = meteora_bin_array_index(active_bin_id);
    let bin_array_pubkeys: Vec<PubkeyBytes> = (-METEORA_BIN_ARRAY_RADIUS
        ..=METEORA_BIN_ARRAY_RADIUS)
        .filter_map(|offset| {
            derive_meteora_bin_array_pda(pool.metadata.pubkey, active_array_index + offset).ok()
        })
        .collect();

    let accounts = fetch_accounts(rpc, &bin_array_pubkeys).await?;
    state.bins.clear();
    for data in accounts.into_iter().flatten() {
        match parse_meteora_bin_array(&data, pool.metadata.pubkey) {
            Ok(mut bins) => state.bins.append(&mut bins),
            Err(error) => log::debug!(
                "Meteora bin array parse failed for pool {}: {error:#}",
                pool.metadata.id
            ),
        }
    }
    state.bins.sort_by_key(|bin| bin.id);
    state.bins.dedup_by_key(|bin| bin.id);

    Ok(pool)
}
