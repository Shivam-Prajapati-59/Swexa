pub mod adapters;
pub mod api;
pub mod models;
pub mod types;

use axum::{Router, routing::get};
use types::AppState;

#[tokio::main]
async fn main() {
    let app_state = AppState::new().await;

    let app = Router::new()
        .route("/pools", get(api::get_all_pools))
        .with_state(app_state);

    println!("Server running on http://127.0.0.1:8000");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
