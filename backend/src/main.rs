mod adapters;
mod api;
mod config;
mod engine;
mod routing;
mod simulation;
mod types;

use adapters::fetch_all_pools;
use api::{AppState, build_router};
use engine::quote::QuoteEngine;
use std::{env, sync::Arc};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let rpc_url = env::var("RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

    let pools = fetch_all_pools(None).await;
    let state = Arc::new(AppState::from_pools_with_rpc(pools, &rpc_url));
    let app = build_router(state.clone());

    println!("Swexa backend bootstrapped (Phase 2: SVM-backed simulation)");
    println!("RPC endpoint: {rpc_url}");
    println!("Pools loaded: {}", state.pools.len());
    println!("Graph tokens: {}", state.stats.token_count);
    println!("Graph directed edges: {}", state.stats.directed_edge_count);
    println!("Graph valid: {}", state.validation.is_valid);

    if !state.validation.is_valid {
        println!(
            "Validation details: missing mappings={}, mismatched weights={}, stale mappings={}, invalid endpoints={}, missing reverse edges={}",
            state.validation.missing_node_mappings,
            state.validation.mismatched_node_weights,
            state.validation.stale_node_mappings,
            state.validation.invalid_edge_endpoints,
            state.validation.missing_reverse_edges
        );
    }

    // ── Smoke Test: Heuristic (fast sanity check) ──────────────────────────
    let smoke_amount_in = 1_000_000u64;
    let smoke_quote = state.pools.iter().find_map(|pool| {
        if pool.token_a.mint == pool.token_b.mint {
            return None;
        }

        let routes = state
            .graph
            .find_routes(&pool.token_a.mint, &pool.token_b.mint, 1, Some(1));
        let route = routes.first()?;
        let quote = QuoteEngine::quote_route(route, smoke_amount_in)?;

        Some((
            pool.token_a.symbol.clone(),
            pool.token_b.symbol.clone(),
            route.len(),
            quote.estimated_amount_out,
            quote.price_impact_bps,
        ))
    });

    if let Some((from_symbol, to_symbol, hops, amount_out, impact_bps)) = smoke_quote {
        println!(
            "Heuristic smoke test: {smoke_amount_in} {from_symbol} -> {amount_out} {to_symbol} (hops={hops}, impact={}bps)",
            impact_bps
        );
    } else {
        println!("Heuristic smoke test skipped: no quoteable route found.");
    }

    // ── Smoke Test: Simulated Engine ───────────────────────────────────────
    println!("\nSimulated quote engine: READY (will activate on /quote requests)");
    println!("  Use ?heuristic_only=true to force heuristic mode");
    println!("  The engine auto-falls-back to heuristic if simulation fails");

    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;

    println!("\nListening on http://127.0.0.1:{port}");
    println!("Endpoints:");
    println!("  GET /health           — Health check");
    println!("  GET /graph/stats      — Graph statistics");
    println!("  GET /routes           — Find candidate routes");
    println!("  GET /quote            — Get best quote (SVM simulation + split optimization)");
    axum::serve(listener, app).await?;

    Ok(())
}
