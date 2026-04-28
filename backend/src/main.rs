mod adapters;
mod config;
mod types;

use adapters::fetch_all_pools;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("════════════════════════════════════════════════════════════");
    println!("  Swexa Routing Engine — Pool Discovery");
    println!("════════════════════════════════════════════════════════════\n");

    // Fetch and merge pools from all DEXes (min TVL = $1000).
    let pools = fetch_all_pools(None).await;

    if pools.is_empty() {
        eprintln!("\n⚠ No pools available. Check network connectivity.");
        return Ok(());
    }

    // ── Summary stats ──────────────────────────────────────────────────
    let whirlpool_count = pools
        .iter()
        .filter(|p| p.dex == types::DexProtocol::Whirlpool)
        .count();
    let raydium_count = pools
        .iter()
        .filter(|p| p.dex == types::DexProtocol::Raydium)
        .count();

    println!("\n── Breakdown ──────────────────────────────────────────");
    println!("  Whirlpool pools : {}", whirlpool_count);
    println!("  Raydium pools   : {}", raydium_count);
    println!("  Total edges     : {}", pools.len());

    // ── Print a few sample edges ───────────────────────────────────────
    println!("\n── Sample Edges ───────────────────────────────────────");
    for edge in pools.iter().take(5) {
        println!(
            "  [{:<10}] {} ({}) ↔ {} ({}) | TVL ${:.0} | Fee {:.4}% | {}",
            edge.dex.to_string(),
            edge.token_a.symbol,
            &edge.token_a.mint[..8],
            edge.token_b.symbol,
            &edge.token_b.mint[..8],
            edge.tvl,
            edge.fee_rate * 100.0,
            edge.pool_type,
        );
    }
    println!("  ...\n");

    Ok(())
}
