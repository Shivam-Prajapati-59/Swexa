use crate::api::{AppState, build_router};
use crate::types::{DexProtocol, PoolEdge, PoolType, TokenMint};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::util::ServiceExt;

fn token(mint: &str) -> TokenMint {
    TokenMint {
        mint: mint.to_string(),
        symbol: mint.to_string(),
        decimals: 9,
    }
}

fn pool(address: &str, a: &str, b: &str) -> PoolEdge {
    PoolEdge {
        address: address.to_string(),
        dex: DexProtocol::Raydium,
        token_a: token(a),
        token_b: token(b),
        fee_rate: 0.003,
        tvl: if address.contains("deep") {
            100_000.0
        } else {
            1_000.0
        },
        pool_type: PoolType::Amm,
    }
}

fn app() -> axum::Router {
    let pools = vec![
        pool("ab", "A", "B"),
        pool("bc", "B", "C"),
        pool("cd", "C", "D"),
        pool("de", "D", "E"),
        pool("ac", "A", "C"),
        pool("ce", "C", "E"),
        pool("deep-ae", "A", "E"),
    ];
    build_router(Arc::new(AppState::from_pools(pools)))
}

#[tokio::test]
async fn health_endpoint_reports_ready_graph() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["graph_ready"], true);
}

#[tokio::test]
async fn routes_endpoint_returns_four_hop_candidate() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/routes?source_mint=A&target_mint=E&max_hops=4&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let routes = json["routes"].as_array().unwrap();

    assert!(routes.iter().any(|route| route["hops"] == 4));
}

#[tokio::test]
async fn quote_endpoint_returns_best_route() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/quote?source_mint=A&target_mint=E&amount_in=1000&max_hops=4&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["amount_in"], 1000);
    assert!(json["candidate_route_count"].as_u64().unwrap() > 0);
    assert_eq!(
        json["best_route"]["path"][0]["pool_address"],
        serde_json::Value::String("deep-ae".to_string())
    );
}

#[tokio::test]
async fn routes_endpoint_rejects_identical_tokens() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/routes?source_mint=A&target_mint=A")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
