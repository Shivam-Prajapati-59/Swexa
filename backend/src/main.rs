mod adapters;
use adapters::whirlpool::{fetch_all_whirlpools, fetch_whirlpool_pool};

fn main() -> anyhow::Result<()> {
    let rpc = "https://api.mainnet-beta.solana.com";

    // Ensure you are passing a valid Base58 string!
    // For example, this is the SOL/USDC Whirlpool address:
    let pool_address = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE";

    let pool_data = fetch_whirlpool_pool(rpc, pool_address)?;

    // FIX: Actively reading the fields fixes the "fields are never read" warning.
    println!("Liquidity: {}", pool_data.liquidity);
    println!("Sqrt Price: {}", pool_data.sqrt_price_x64);
    println!("Tick Current Index: {}", pool_data.tick_current_index);
    println!("Token A: {}", pool_data.token_mint_a);
    println!("Token B: {}", pool_data.token_mint_b);

    match fetch_all_whirlpools(rpc) {
        Ok(all_pools) => {
            println!("Total whirlpool accounts fetched: {}", all_pools.len());
        }
        Err(err) => {
            eprintln!("Could not fetch all Whirlpool pools from this RPC endpoint: {err}");
            eprintln!(
                "Tip: use a dedicated RPC provider URL (Helius/QuickNode/Triton) for large program scans."
            );
        }
    }

    Ok(())
}
