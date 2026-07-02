use crate::services::pool_service::{
    DEFAULT_POOL_PAGE_SIZE, PoolPage, cached_pool_page, refresh_pool_cache,
};
use crate::types::AppState;
use axum::extract::{Query, State};
use axum::{http::StatusCode, response::Json};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PoolsQuery {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub refresh: Option<bool>,
}

/// Returns a cached, paginated pool list.
///
/// Pool refresh is metadata-only and explicit via `refresh=true`; quote-grade
/// RPC hydration is done later for selected route candidates.
pub async fn get_all_pools(
    State(state): State<AppState>,
    Query(params): Query<PoolsQuery>,
) -> Result<Json<PoolPage>, (StatusCode, String)> {
    let should_refresh = params.refresh.unwrap_or(false) || state.pools.read().await.is_empty();

    if should_refresh {
        refresh_pool_cache(&state).await.map_err(|err| {
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to refresh DEX pools: {err:#}"),
            )
        })?;
    }

    let page = params.page.unwrap_or(1);
    let page_size = params.page_size.unwrap_or(DEFAULT_POOL_PAGE_SIZE);
    Ok(Json(cached_pool_page(&state, page, page_size).await))
}
