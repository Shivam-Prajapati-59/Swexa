use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const WHIRLPOOL_PROGRAM_ID: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

// Added #[allow(dead_code)] if you're building a library and want to
// silence the warning without necessarily reading every field.
#[allow(dead_code)]
#[derive(Debug)]
pub struct WhirlpoolPoolData {
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current_index: i32,
    pub token_mint_a: Pubkey,
    pub token_mint_b: Pubkey,
}

pub fn fetch_whirlpool_pool(
    rpc_url: &str,
    pool_address: &str,
) -> anyhow::Result<WhirlpoolPoolData> {
    let client = RpcClient::new(rpc_url.to_string());

    // FIX: Trim whitespace/newlines and add detailed error context
    let pool_pubkey = Pubkey::from_str(pool_address.trim()).map_err(|_| {
        anyhow::anyhow!("Failed to parse pool address as Base58: '{}'", pool_address)
    })?;

    let program_id = Pubkey::from_str(WHIRLPOOL_PROGRAM_ID)?;

    let account = client.get_account(&pool_pubkey)?;

    if account.owner != program_id {
        anyhow::bail!("Account is not owned by Whirlpool program");
    }

    let data = &account.data;
    if data.len() < 213 {
        anyhow::bail!("Account data too short");
    }

    let liquidity = u128::from_le_bytes(data[49..65].try_into()?);
    let sqrt_price_x64 = u128::from_le_bytes(data[65..81].try_into()?);
    let tick_current_index = i32::from_le_bytes(data[81..85].try_into()?);
    let token_mint_a = Pubkey::new_from_array(data[101..133].try_into()?);
    let token_mint_b = Pubkey::new_from_array(data[181..213].try_into()?);

    Ok(WhirlpoolPoolData {
        liquidity,
        sqrt_price_x64,
        tick_current_index,
        token_mint_a,
        token_mint_b,
    })
}

#[allow(deprecated)]
pub fn fetch_all_whirlpools(rpc_url: &str) -> anyhow::Result<Vec<(Pubkey, WhirlpoolPoolData)>> {
    let client = RpcClient::new(rpc_url.to_string());
    let program_id = Pubkey::from_str(WHIRLPOOL_PROGRAM_ID)?;

    println!("Fetching all pools... (This might take a moment)");

    // Fetch all Whirlpool-owned accounts as raw binary data.
    let accounts = client.get_program_accounts(&program_id)?;
    let mut all_pools = Vec::new();

    // 3. Iterate and parse each pool
    for (pubkey, account) in accounts {
        let data = &account.data;

        if data.len() == 656 {
            let liquidity = u128::from_le_bytes(data[49..65].try_into()?);
            let sqrt_price_x64 = u128::from_le_bytes(data[65..81].try_into()?);
            let tick_current_index = i32::from_le_bytes(data[81..85].try_into()?);
            let token_mint_a = Pubkey::new_from_array(data[101..133].try_into()?);
            let token_mint_b = Pubkey::new_from_array(data[181..213].try_into()?);

            let pool_data = WhirlpoolPoolData {
                liquidity,
                sqrt_price_x64,
                tick_current_index,
                token_mint_a,
                token_mint_b,
            };

            all_pools.push((pubkey, pool_data));
        }
    }

    println!("Successfully fetched {} pools.", all_pools.len());
    Ok(all_pools)
}
