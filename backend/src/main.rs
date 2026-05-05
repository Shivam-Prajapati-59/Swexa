mod adapters;
mod api;
mod config;
mod processor;
mod types;

use adapters::fetch_all_pools;
use api::{AppState, build_router};
use std::{env, sync::Arc};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pools = fetch_all_pools(None).await;
    let state = Arc::new(AppState::from_pools(pools));
    let app = build_router(state.clone());

    println!("Swexa Step 1 backend bootstrapped");
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

    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;

    println!("Listening on http://127.0.0.1:{port}");
    axum::serve(listener, app).await?;

    Ok(())
}
