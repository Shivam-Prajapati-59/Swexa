use crate::{adapters, models::pool::Pool, types::AppState};
use axum::{extract::State, http::StatusCode, response::Json};
use reqwest::Client;

pub async fn get_all_pools(
    State(state): State<AppState>,
) -> Result<Json<Vec<Pool>>, (StatusCode, String)> {
    let client = Client::new();
    let pools = adapters::fetch_all_pools(&client, None)
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to fetch DEX pools: {err:#}"),
            )
        })?;

    let mut cached_pools = state.pools.write().await;
    *cached_pools = pools.clone();

    Ok(Json(pools))
}
