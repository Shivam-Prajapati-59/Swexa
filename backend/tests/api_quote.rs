use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use backend::build_app;
use backend::models::pool::{
    CpmmState, DexProtocol, Pool, PoolData, PoolMetadata, PoolStatus, PoolToken, PoolType,
    PubkeyBytes,
};
use backend::types::AppState;
use serde_json::Value;
use std::sync::atomic::Ordering;
use tower::ServiceExt;

const SOL_MINT: PubkeyBytes = PubkeyBytes([1u8; 32]);
const USDC_MINT: PubkeyBytes = PubkeyBytes([2u8; 32]);

fn pubkey_string(pubkey: PubkeyBytes) -> String {
    solana_sdk::pubkey::Pubkey::new_from_array(pubkey.0).to_string()
}

fn cpmm_pool() -> Pool {
    Pool {
        metadata: PoolMetadata {
            id: 7,
            pubkey: PubkeyBytes([9u8; 32]),
            protocol: DexProtocol::Raydium,
            pool_type: PoolType::Cpmm,
            status: PoolStatus::Active,
            token_a: PoolToken {
                mint: SOL_MINT,
                name: "Wrapped SOL".to_string(),
                symbol: "SOL".to_string(),
                decimals: 9,
                vault: Some(PubkeyBytes([3u8; 32])),
            },
            token_b: PoolToken {
                mint: USDC_MINT,
                name: "USD Coin".to_string(),
                symbol: "USDC".to_string(),
                decimals: 6,
                vault: Some(PubkeyBytes([4u8; 32])),
            },
        },
        data: PoolData::Cpmm(CpmmState {
            reserve_a: 1_000_000_000_000,
            reserve_b: 75_000_000_000,
        }),
        fee_rate: 3_000,
        tvl: Some(1_000_000.0),
        last_updated_slot: Some(1),
    }
}

async fn app_with_mock_pools() -> axum::Router {
    let state = AppState::new().await;
    {
        let mut pools = state.pools.write().await;
        pools.push(cpmm_pool());
    }
    state.pool_generation.fetch_add(1, Ordering::Release);
    build_app(state)
}

#[tokio::test]
async fn quote_rejects_zero_amount() {
    let app = app_with_mock_pools().await;
    let uri = format!(
        "/api/quote?input_mint={}&output_mint={}&amount=0",
        pubkey_string(SOL_MINT),
        pubkey_string(USDC_MINT)
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn quote_accepts_large_u128_amount_as_string_query() {
    let app = app_with_mock_pools().await;
    let uri = format!(
        "/api/quote?inputMint={}&outputMint={}&amount=1000000000",
        pubkey_string(SOL_MINT),
        pubkey_string(USDC_MINT)
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["amount_in"], 1_000_000_000u64);
    assert!(json["best_routes"].as_array().unwrap().len() >= 1);
    assert!(
        json["best_routes"][0]["estimated_amount_out"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(json["best_routes"][0]["route"]["hops"], 1);
}

#[tokio::test]
async fn route_alias_uses_same_quote_handler() {
    let app = app_with_mock_pools().await;
    let uri = format!(
        "/api/route?input_mint={}&output_mint={}&amount=1000000000",
        pubkey_string(SOL_MINT),
        pubkey_string(USDC_MINT)
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(!json["best_routes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn allroutes_returns_graph_candidates() {
    let app = app_with_mock_pools().await;
    let uri = format!(
        "/api/allroutes?input_mint={}&output_mint={}&max_hops=2&max_routes=10",
        pubkey_string(SOL_MINT),
        pubkey_string(USDC_MINT)
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["routes_found"], 1);
    assert_eq!(json["routes"][0]["hops"], 1);
}

#[tokio::test]
async fn pools_endpoint_returns_cached_paginated_pools() {
    let app = app_with_mock_pools().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pools?page=1&page_size=1")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["total"], 1);
    assert_eq!(json["page"], 1);
    assert_eq!(json["page_size"], 1);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
}
