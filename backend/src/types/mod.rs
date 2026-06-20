use crate::models::pool::Pool;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub pools: Arc<RwLock<Vec<Pool>>>,
}

impl AppState {
    pub async fn new() -> Self {
        Self {
            pools: Arc::new(RwLock::new(Vec::new())),
        }
    }
}
