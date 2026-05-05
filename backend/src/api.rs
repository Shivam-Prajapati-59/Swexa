use crate::processor::{GraphStats, GraphValidationReport, MAX_SUPPORTED_HOPS, RouteGraph};
use crate::types::{DexProtocol, PoolEdge, PoolType};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub struct AppState {
    pub pools: Vec<PoolEdge>,
    pub graph: RouteGraph,
    pub stats: GraphStats,
    pub validation: GraphValidationReport,
}

impl AppState {
    pub fn from_pools(pools: Vec<PoolEdge>) -> Self {
        let graph = RouteGraph::new(&pools);
        let stats = graph.stats();
        let validation = graph.validate();

        Self {
            pools,
            graph,
            stats,
            validation,
        }
    }
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/graph/stats", get(graph_stats))
        .route("/routes", get(find_routes))
        .with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    pool_count: usize,
    max_supported_hops: usize,
    graph_ready: bool,
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        pool_count: state.pools.len(),
        max_supported_hops: MAX_SUPPORTED_HOPS,
        graph_ready: state.validation.is_valid,
    })
}

#[derive(Serialize)]
struct GraphStatsResponse {
    graph: GraphStats,
    validation: GraphValidationReport,
    dex_pool_counts: BTreeMap<&'static str, usize>,
    pool_type_counts: BTreeMap<&'static str, usize>,
}

async fn graph_stats(State(state): State<Arc<AppState>>) -> Json<GraphStatsResponse> {
    let mut dex_pool_counts = BTreeMap::new();
    let mut pool_type_counts = BTreeMap::new();

    for pool in &state.pools {
        *dex_pool_counts.entry(dex_label(pool.dex)).or_default() += 1;
        *pool_type_counts
            .entry(pool_type_label(pool.pool_type))
            .or_default() += 1;
    }

    Json(GraphStatsResponse {
        graph: state.stats.clone(),
        validation: state.validation.clone(),
        dex_pool_counts,
        pool_type_counts,
    })
}

#[derive(Debug, Deserialize)]
struct RouteQuery {
    source_mint: String,
    target_mint: String,
    #[serde(default = "default_max_hops")]
    max_hops: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Serialize)]
struct RoutesResponse {
    source_mint: String,
    target_mint: String,
    requested_max_hops: usize,
    effective_max_hops: usize,
    returned_route_count: usize,
    routes: Vec<RouteSummary>,
}

#[derive(Serialize)]
struct RouteSummary {
    hops: usize,
    total_fee_rate: f64,
    estimated_total_fee_bps: u64,
    path: Vec<RouteHop>,
}

#[derive(Serialize)]
struct RouteHop {
    pool_address: String,
    dex: &'static str,
    pool_type: &'static str,
    from_mint: String,
    from_symbol: String,
    to_mint: String,
    to_symbol: String,
    fee_rate: f64,
    tvl: f64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

async fn find_routes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RouteQuery>,
) -> impl IntoResponse {
    if query.source_mint == query.target_mint {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "source_mint and target_mint must be different".to_string(),
            }),
        ));
    }

    let effective_max_hops = query.max_hops.min(MAX_SUPPORTED_HOPS);
    let limit = query.limit.clamp(1, 100);
    let routes = state.graph.find_routes(
        &query.source_mint,
        &query.target_mint,
        effective_max_hops,
        Some(limit),
    );

    let response = RoutesResponse {
        source_mint: query.source_mint.clone(),
        target_mint: query.target_mint.clone(),
        requested_max_hops: query.max_hops,
        effective_max_hops,
        returned_route_count: routes.len(),
        routes: routes
            .iter()
            .map(|route| summarize_route(&query.source_mint, route))
            .collect(),
    };

    Ok(Json(response))
}

fn summarize_route(source_mint: &str, route: &[PoolEdge]) -> RouteSummary {
    let mut current_mint = source_mint.to_string();
    let mut total_fee_rate = 0.0;
    let mut path = Vec::with_capacity(route.len());

    for pool in route {
        let (from_token, to_token) = if pool.token_a.mint == current_mint {
            (&pool.token_a, &pool.token_b)
        } else {
            (&pool.token_b, &pool.token_a)
        };

        total_fee_rate += pool.fee_rate;
        current_mint = to_token.mint.clone();

        path.push(RouteHop {
            pool_address: pool.address.clone(),
            dex: dex_label(pool.dex),
            pool_type: pool_type_label(pool.pool_type),
            from_mint: from_token.mint.clone(),
            from_symbol: from_token.symbol.clone(),
            to_mint: to_token.mint.clone(),
            to_symbol: to_token.symbol.clone(),
            fee_rate: pool.fee_rate,
            tvl: pool.tvl,
        });
    }

    RouteSummary {
        hops: route.len(),
        total_fee_rate,
        estimated_total_fee_bps: (total_fee_rate * 10_000.0).round() as u64,
        path,
    }
}

fn default_max_hops() -> usize {
    MAX_SUPPORTED_HOPS
}

fn default_limit() -> usize {
    20
}

fn dex_label(dex: DexProtocol) -> &'static str {
    match dex {
        DexProtocol::Whirlpool => "whirlpool",
        DexProtocol::Raydium => "raydium",
        DexProtocol::Meteora => "meteora",
    }
}

fn pool_type_label(pool_type: PoolType) -> &'static str {
    match pool_type {
        PoolType::Amm => "amm",
        PoolType::ConcentratedLiquidity => "clmm",
        PoolType::Dlmm => "dlmm",
    }
}

#[cfg(test)]
mod tests {
    use super::{AppState, build_router};
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
            tvl: 1_000.0,
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
}
