mod adapters;
mod config;
mod processor;
mod types;

use adapters::fetch_all_pools;
use processor::RouteGraph;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("════════════════════════════════════════════════════════════");
    println!("  Swexa Routing Engine — Pool Discovery & Graph Construction");
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

    // ── Build the Graph ────────────────────────────────────────────────
    println!("\n── Graph Construction ─────────────────────────────────");
    let graph = RouteGraph::new(&pools);
    println!("✅ Adjacency list built. Unique tokens in graph: {}", graph.mint_to_node.len());

    // ── Test Pathfinding (SOL -> USDC) ─────────────────────────────────
    let wsol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let max_depth = 3;

    println!("\n── Finding Routes (Max Depth: {}) ────────────────────────", max_depth);
    println!("Searching routes from WSOL to USDC...");
    
    let routes = graph.find_routes(wsol_mint, usdc_mint, max_depth);
    
    println!("✅ Found {} possible routes between WSOL and USDC.", routes.len());

    let hop_1_routes: Vec<_> = routes.iter().filter(|r| r.len() == 1).collect();
    let hop_2_routes: Vec<_> = routes.iter().filter(|r| r.len() == 2).collect();
    let hop_3_routes: Vec<_> = routes.iter().filter(|r| r.len() == 3).collect();

    println!("  - 1-hop routes: {}", hop_1_routes.len());
    println!("  - 2-hop routes: {}", hop_2_routes.len());
    println!("  - 3-hop routes: {}", hop_3_routes.len());

    println!("\n── Sample 2-hop Route ──────────────────────────────────");
    if let Some(route) = hop_2_routes.first() {
        let mut current_mint = wsol_mint.to_string();
        for (hop_idx, pool) in route.iter().enumerate() {
            let (input_symbol, output_symbol, next_mint) = if pool.token_a.mint == current_mint {
                (&pool.token_a.symbol, &pool.token_b.symbol, &pool.token_b.mint)
            } else {
                (&pool.token_b.symbol, &pool.token_a.symbol, &pool.token_a.mint)
            };
            println!("    Hop {}: [{:?}] {} ➔ {} (Pool: {})", 
                hop_idx + 1, 
                pool.dex, 
                input_symbol, 
                output_symbol,
                &pool.address[..8]
            );
            current_mint = next_mint.clone();
        }
    }

    println!("\n── Sample 3-hop Route ──────────────────────────────────");
    if let Some(route) = hop_3_routes.first() {
        let mut current_mint = wsol_mint.to_string();
        for (hop_idx, pool) in route.iter().enumerate() {
            let (input_symbol, output_symbol, next_mint) = if pool.token_a.mint == current_mint {
                (&pool.token_a.symbol, &pool.token_b.symbol, &pool.token_b.mint)
            } else {
                (&pool.token_b.symbol, &pool.token_a.symbol, &pool.token_a.mint)
            };
            println!("    Hop {}: [{:?}] {} ➔ {} (Pool: {})", 
                hop_idx + 1, 
                pool.dex, 
                input_symbol, 
                output_symbol,
                &pool.address[..8]
            );
            current_mint = next_mint.clone();
        }
    }
    
    println!("  ...\n");

    Ok(())
}
