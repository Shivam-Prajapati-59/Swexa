use backend::{build_app, types::AppState};

#[tokio::main]
async fn main() {
    let app_state = AppState::new().await;
    let app = build_app(app_state);

    println!("Server running on http://127.0.0.1:8000");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
